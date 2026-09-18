#!/usr/bin/env bash
# A file is at most 500 lines, and no module is named a dumping ground.
# `M-1.27`, `code-structure.md` rules 16, 17, 24.
#
#   scripts/check-file-size.sh
#
# ## Scope
#
# `*.rs` files only, tracked (`git ls-files`), the whole tree — a file that
# grew past the limit three commits ago is exactly as much a violation today
# as one that just crossed it, so this is a property of the tree, not of a
# diff, the same reasoning `check-drift.sh`'s header already gives for the
# same shape of rule. `Cargo.toml`/`README.md`/`AGENTS.md` are in
# `code-structure.md`'s `applies_to` because *other* rules in that document
# govern them (`check-readmes.sh`, `M-1.27`'s sibling); the 500-line limit
# itself is rule 16, grouped under "## Files" with the generated-table and
# match-arm examples that make sense only for source.
#
# ## The allowlist
#
# Rule 17: "The limit has an allowlist, and every entry carries a reason...
# in the script, so adding to it is a diff someone reviews." `ALLOWLIST` below
# has one entry: each is a one-line addition with its reason inline, not a
# separate file.
#
# ⚠️ **Two kinds of entry, and they bound growth differently** (`M5.42`,
# `ADR-0040`). A **numeric** entry is a deferral — the file is over the limit,
# it should not be, and the count it was granted at is what stops it quietly
# becoming a licence to grow. A **`list`** entry is rule 17's own
# generated-table case, and it carries no count because a count would have to
# be raised by every commit that adds an entry to the list; what bounds it is
# structural instead, and the section on `ALLOWLIST` below says exactly what.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

if ! has_rust; then
  skip "file size (no Cargo.toml yet)"
  finish
fi

# ⚠️ `FILE_LINE_LIMIT`, not `LIMIT` — the name is load-bearing (`M0.15`).
# `check-drift.sh`'s THRESHOLD_RE matches `_limit`, so the bare name was
# invisible to it: made settable from the environment, it passed the whole
# suite green. `M1.35`, which also lists it in `m0-complete.sh`'s
# `NFR_CONSTANTS`, so changing the value fails that gate — ⚠️ at the milestone
# boundary, since nothing on the commit path invokes it.
#
# ⚠️ The env-read form is deliberately **not** written out here. `check-drift.sh`
# fails any line carrying both a threshold name and an environment read, and
# excludes only itself — so a worked example in this file is one comment reflow
# away from a permanent failure with no suppression available.
FILE_LINE_LIMIT=500

# path (relative to repo root) -> "<granted line count>|<why>" or
# "list|<why>". ⚠️ **Both halves, and the first one decides which kind of
# exemption it is**: an entry with neither a numeric prefix nor `list` is
# refused below rather than treated as an unbounded exemption, which is what a
# bare reason silently became when `M5.43` added the count.
#
# ⚠️ **A numeric entry is a deferral**: the file is over the limit, someone
# wrote down the count it was granted at, and a row says when it comes out.
# Exceeding the granted count fails, with the entry's own reason printed.
#
# ⚠️ **A `list` entry is rule 17's own case, and it has no number** (`M5.42`,
# `ADR-0040`). Rule 16's limit measures a module you can hold in your head; a
# file that is one flat list — a generated table, an enum of independent
# variants — is not that, and giving it a number would mean raising the number
# on the commit that adds the forty-seventh entry, which is the churn `M5.42`
# was opened by. What keeps it honest instead is **structural**: the file must
# hold exactly one top-level item, and it must not be a module. The moment it
# grows an `impl`, a helper or a second type, it is a module again and the
# limit bites — so the exemption cannot outlive the reason it was granted for,
# which no line count could check.
declare -A ALLOWLIST=(
  [crates/oqueue-core/src/error.rs]="list|one error enum is one list of independent variants; ADR-0040 keeps it flat because splitting it into sub-enums would let a new variant land without breaking an exhaustive match"
)

# Top-level item keywords. A file exempt as a `list` may hold exactly one, and
# `mod` counts as one on purpose: a file with a module in it is a module.
#
# ⚠️ **The modifiers are the whole difficulty, and a first version missed six
# forms.** `impl<T> From<T> for Error {}`, `unsafe impl Send for Error {}`,
# `async fn`, `pub async fn`, `pub unsafe fn` and `pub(crate) async fn` all
# passed a pattern that allowed only `pub`/`pub(...)` and demanded a space
# immediately after the keyword — and a `From` impl is the most likely thing an
# error enum ever grows. So visibility and any run of `async`/`unsafe`/
# `const`/`extern`/`default` are consumed before the keyword, and the keyword
# is followed by any non-identifier character or the line's end, which admits
# `impl<T>` and `macro_rules!`.
#
# ⚠️ **`const` is in both halves, and that is not redundancy**: `const fn` is a
# modifier and `const MAX: usize` is an item, so a pattern that moved it into
# the modifier run alone stopped matching a bare `const` — which round two
# found, with `pub const MAX_LEN: usize = 256;` taking the exemption. `extern`
# has the same shape one step further, since `extern "C" fn` puts an ABI string
# between the modifier and the keyword.
#
# ⚠️ **No `\b`**, which this repository forbids outright: GNU and BSD grep
# disagree about it, and a pattern that silently matches nothing is the
# fail-open every leg here is written against.
# ⚠️ **`extern crate foo;` and an `extern "C" { … }` block are not items to
# this pattern, and that is a decision** (`M5.56`). The `extern` branch above
# exists for `extern "C" fn` — a modifier run before a keyword — and neither
# of those two has a keyword from the alternation after it: `extern crate foo;`
# ends there, and a block's contents are indented past the `^` anchor. So a
# file allowlisted as a list may carry either without losing its exemption,
# which is the right answer for a `#[link]` shim beside a generated table. ⚠️
# **`tests/gates/negative.sh` holds it**: two cases plant one of these beside a
# `pub mod inner;` and assert the count is **two**, so making `extern` count
# reds them.
ITEM_RE='^(pub(\([^)]*\))?[[:space:]]+)?((async|unsafe|const|default)[[:space:]]+|extern([[:space:]]+"[^"]*")?[[:space:]]+)*(enum|struct|union|trait|impl|fn|mod|const|static|type|macro_rules)([^A-Za-z0-9_]|$)'

# Counts a `list` file's top-level items, failing if there is not exactly one,
# or if one of them is a module declaration.
#
# ⚠️ **`^` anchored, so nested items do not count.** An `impl` inside a
# function body is indented; a top-level one is not. That is a grep's reading
# of Rust rather than a parser's, and it is the same reading every other gate
# in this repository makes — deliberately, since a parser here would be a
# second compiler to keep correct.
list_exemption_holds() {
  local file="$1" items
  items="$(grep -cE "$ITEM_RE" "$file" || true)"
  if (( items != 1 )); then
    fail "$file: allowlisted as a list, but holds $items top-level item(s), not 1"
    note "a list exemption is rule 17's generated-table case; a file with more"
    note "than one item is a module, and rule 16's limit is what measures it"
    return 1
  fi
  if grep -qE '^(pub(\([^)]*\))?[[:space:]]+)?mod[[:space:]]' "$file"; then
    fail "$file: allowlisted as a list, but declares a module"
    return 1
  fi
  return 0
}

# Module names that are a place to put things nobody decided where else to
# put, not a concept — rule 24. Matched against the file's stem, so
# `util.rs` and `crates/x/src/util.rs` both hit and `evaluate.rs` does not.
FORBIDDEN_STEMS=(util utils common commons helpers helper misc)

mapfile -t files < <(git ls-files -- '*.rs')

if (( ${#files[@]} == 0 )); then
  skip "file size (no .rs files tracked yet)"
  finish
fi

checked=0
violations=0
for f in "${files[@]}"; do
  [[ -f "$f" ]] || continue
  checked=$((checked + 1))

  stem="$(basename "$f" .rs)"
  for bad in "${FORBIDDEN_STEMS[@]}"; do
    if [[ "$stem" == "$bad" ]]; then
      fail "$f: module named '$stem' -- util/common/helpers/misc is a dumping ground, not a concept"
      note "rule 24: split by what the code does, not into a place with no name"
      violations=$((violations + 1))
      break
    fi
  done

  lines="$(wc -l < "$f")"
  lines="${lines//[[:space:]]/}"

  if (( lines > FILE_LINE_LIMIT )); then
    if [[ -n "${ALLOWLIST[$f]:-}" ]]; then
      entry="${ALLOWLIST[$f]}"
      granted="${entry%%|*}"
      why="${entry#*|}"
      if [[ "$granted" == "list" ]]; then
        if list_exemption_holds "$f"; then
          note "$f: $lines lines, over $FILE_LINE_LIMIT, allowlisted as a list -- $why"
        else
          note "$why"
          note "the exemption's own condition no longer holds, so the limit applies"
          violations=$((violations + 1))
        fi
      elif [[ ! "$granted" =~ ^[0-9]+$ ]]; then
        # ⚠️ **A malformed entry fails rather than exempting.** Without this the
        # arithmetic below errors, evaluates false, and the file is exempted
        # with no ceiling at all -- the gate off for exactly the entry someone
        # got wrong.
        fail "$f: allowlist entry must be \"<granted line count>|<reason>\" or \"list|<reason>\", got: $entry"
        violations=$((violations + 1))
      elif (( lines > granted )); then
        fail "$f: $lines lines, over the $granted it was allowlisted at"
        note "$why"
        note "an exemption is a deferral, not a licence to grow -- raise the count only with the row that retires it"
        violations=$((violations + 1))
      else
        note "$f: $lines lines, over $FILE_LINE_LIMIT, allowlisted at $granted -- $why"
      fi
    else
      fail "$f: $lines lines, over the $FILE_LINE_LIMIT-line limit"
      note "rule 18: a design signal, not a formatting problem -- split by concept"
      note "rule 17: legitimate cases (generated tables, exhaustive match arms) go in this script's ALLOWLIST, with a reason"
      violations=$((violations + 1))
    fi
  fi
done

if (( checked == 0 )); then
  skip "file size (no readable .rs files)"
  finish
fi

# ⚠️ Without this, a violation found earlier in the loop still let the run
# end on `ok`, printing a passing summary alongside its own `FAIL` lines —
# the exact contradiction `check-milestone-review.sh`'s header already
# warns about, and the reason `finish` is called explicitly here rather
# than falling through.
if (( violations > 0 )); then
  finish
fi

ok "file size ($checked .rs file(s) checked)"
finish
