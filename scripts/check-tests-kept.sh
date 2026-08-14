#!/usr/bin/env bash
# A test removed without saying why fails. `M-1.7`.
#
#   scripts/check-tests-kept.sh <file>    the commit-msg hook path
#   scripts/check-tests-kept.sh           checks HEAD against its parent instead
#
# Non-negotiable 2's other half: never delete a test to make a check pass.
# This cannot tell *why* a test disappeared — only that it did, and whether
# the commit says so. A test genuinely made obsolete by a real design change
# is a legitimate deletion; the rule is that it is named, not that it is
# forbidden, mirroring `check-commit-msg.sh`'s posture: this checks that a
# thing was said, never whether it was true.
#
# ## What counts as "removed"
#
# A function that had a `#[test]`-shaped attribute (`#[test]`,
# `#[tokio::test]`, or anything else whose attribute contains the substring
# `test`) in the old version of a file and is absent — under the same name —
# in *that file's* new version. A file deleted outright loses every test it
# held. This is a per-file comparison with no cross-file matching, so ⚠️ a
# test *moved* to another file — even verbatim, even under the same name —
# is still flagged in the file it disappeared from: the gate has no notion
# of "this test still exists, just elsewhere," only "this file's test list
# shrank." That is the safer direction to be wrong in, and it means a
# genuine move still needs its `Removes-test:` trailer. A test *renamed in
# place* is the one case this cannot distinguish from a real deletion plus
# a real addition — see below.
#
# ## The escape hatch, and why it is a trailer rather than a ban
#
# A commit that removes a test adds a line anywhere in its message matching
# `Removes-test: <reason>`. Non-blank, non-comment — a `# Removes-test: ...`
# hint left over from a template does not count, the same way a comment line
# is never mistaken for a commit subject in `check-commit-msg.sh`.
#
# ## What this deliberately does not catch
#
# - **A renamed test.** The extraction is by name; a rename is one removal
#   and one addition, and the removal still demands a trailer. Overclaiming
#   parity here (guessing a rename by heuristic) is exactly the kind of
#   overreach `docs/researches/19` §3.2 warns turns a gate into noise.
# - **A test whose body was hollowed out but whose name and attribute
#   survive.** That test still exists as far as this script or a compiler can
#   tell; catching a test that runs but asserts nothing is `testing.md` rule
#   15's job (mutation testing), not this gate's.
# - **Anything outside `.rs` files.** oqueue has no other test-bearing source
#   yet; this is the same bootstrap gap every content-scanning gate in this
#   repo has today.
# - **A multi-line attribute whose *first* `]` is not its closing one** — a
#   nested `[...]` inside a string or another bracketed sub-expression before
#   the real close, e.g. `doc = "see [foo]"` on an intermediate line of a
#   `#[cfg_attr(...)]` block. The continuation tracker closes on the first
#   `]` it sees, not a balanced count, so it can end the "still inside an
#   attribute" state early. Rare in practice — Kafka protocol code and this
#   project's own style favor single-line attributes — and a real parser is
#   what closes it properly, not a deeper regex.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# Print one test name per line found in Rust source given on stdin.
#
# Heuristic, not a parser: an attribute containing "test" sets a pending
# flag once it closes — checked across *every* line the attribute spans, not
# only its opening one, so a rustfmt-wrapped `#[tokio::test(\n  flavor =
# "multi_thread"\n)]` is recognized exactly like `#[tokio::test]` on one
# line. The flag survives a doc comment, a plain comment, a blank line, a
# second stacked attribute, and a multi-line attribute's own continuation —
# because none of those can themselves contain a `fn`, so none of them can
# be the unrelated code the flag exists to protect against. Anything else
# clears it: an attribute that never reaches a function does not falsely tag
# the next unrelated `fn`. The next `fn name` line consumes the flag and
# reports the name.
extract_tests() {
  awk '
    in_attr {
      if ($0 ~ /test/) attr_has_test = 1
      if ($0 ~ /\]/) {
        in_attr = 0
        if (attr_has_test) pending = 1
        attr_has_test = 0
      }
      next
    }
    /^[[:space:]]*#\[.*test.*\]/ { pending = 1; next }
    /^[[:space:]]*#\[.*\]/       { next }
    /^[[:space:]]*#\[/           {
      in_attr = 1
      attr_has_test = ($0 ~ /test/) ? 1 : 0
      next
    }
    /^[[:space:]]*(\/\/\/|\/\/!|\/\/)/ { next }
    /^[[:space:]]*$/             { next }
    /^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/ {
      if (pending) {
        line = $0
        match(line, /fn[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/)
        name = substr(line, RSTART + 3, RLENGTH - 3)
        gsub(/^[[:space:]]+|[[:space:]]+$/, "", name)
        print name
      }
      pending = 0
      next
    }
    { pending = 0 }
  '
}

MSG_FILE="${1:-}"

if [[ -n "$MSG_FILE" ]]; then
  [[ -f "$MSG_FILE" ]] || { fail "commit message file not found: $MSG_FILE"; finish; }
  # Staged changes about to become the new commit: old = HEAD, new = index.
  if git rev-parse --verify HEAD >/dev/null 2>&1; then
    old_ref="HEAD"
  else
    old_ref=""   # root commit: nothing to compare against, nothing removed
  fi
  mapfile -t changed < <(git diff --cached --name-only --diff-filter=MD -- '*.rs' 2>/dev/null)
  new_of() { git show ":$1" 2>/dev/null || true; }
  message="$(cat "$MSG_FILE")"
  source_desc="staged commit message"
else
  if ! git rev-parse --verify HEAD >/dev/null 2>&1; then
    skip "test removal (no HEAD yet)"
    finish
  fi
  if ! git rev-parse --verify HEAD~1 >/dev/null 2>&1; then
    skip "test removal (HEAD is the root commit, nothing to compare against)"
    finish
  fi
  old_ref="HEAD~1"
  mapfile -t changed < <(git diff --name-only --diff-filter=MD HEAD~1 HEAD -- '*.rs' 2>/dev/null)
  new_of() { git show "HEAD:$1" 2>/dev/null || true; }
  message="$(git log -1 --pretty=%B)"
  source_desc="HEAD"
fi

if [[ -z "$old_ref" || ${#changed[@]} -eq 0 ]]; then
  skip "test removal (no modified or deleted .rs files)"
  finish
fi

declare -a removed_report=()

for f in "${changed[@]}"; do
  [[ -n "$f" ]] || continue
  old_tests="$(git show "$old_ref:$f" 2>/dev/null | extract_tests || true)"
  [[ -n "$old_tests" ]] || continue
  new_tests="$(new_of "$f" | extract_tests || true)"

  while IFS= read -r t; do
    [[ -n "$t" ]] || continue
    if ! grep -qxF "$t" <<< "$new_tests"; then
      removed_report+=("$f: $t")
    fi
  done <<< "$old_tests"
done

if (( ${#removed_report[@]} == 0 )); then
  ok "no test was removed"
  finish
fi

for r in "${removed_report[@]}"; do
  note "removed: $r"
done

# A comment-only line never satisfies the trailer, whichever cleanup mode git
# applies to the real commit — see check-commit-msg.sh for why this script
# cannot know which mode is in play, and why treating a `#`-prefixed line as
# real text either way is the safe direction here.
trailer="$(printf '%s\n' "$message" | grep -E '^[[:space:]]*Removes-test:[[:space:]]*\S' | grep -v '^[[:space:]]*#' || true)"

if [[ -n "$trailer" ]]; then
  ok "${#removed_report[@]} test(s) removed, and the commit says why"
  note "$(printf '%s' "$trailer" | head -1)"
else
  fail "${#removed_report[@]} test(s) removed with no 'Removes-test:' trailer"
  note "source: $source_desc"
  note "add a line: Removes-test: <why this test no longer applies>"
fi

finish
