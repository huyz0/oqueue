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
# in the script, so adding to it is a diff someone reviews." `ALLOWLIST`
# below is empty because no crate exists yet — the first entry, when one is
# needed, is a one-line addition with its reason inline, not a separate file.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

if ! has_rust; then
  skip "file size (no Cargo.toml yet)"
  finish
fi

LIMIT=500

# path (relative to repo root) -> why it is allowed past the limit. Empty
# until a real crate needs an entry.
declare -A ALLOWLIST=()

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

  if (( lines > LIMIT )); then
    if [[ -n "${ALLOWLIST[$f]:-}" ]]; then
      note "$f: $lines lines, over $LIMIT, allowlisted -- ${ALLOWLIST[$f]}"
    else
      fail "$f: $lines lines, over the $LIMIT-line limit"
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
