#!/usr/bin/env bash
# The staged change has been reviewed by someone other than its author. `M-1.9`.
#
#   scripts/check-reviewed.sh     the pre-commit gate
#
# ## What this makes true
#
# Non-negotiable 4. The verdict lives at `target/review/<sha256-of-staged-diff>.json`,
# so this gate recomputes the hash rather than trusting a claim. Amend one byte
# after the review and the artifact no longer matches — the commit is refused,
# and the only way through is to review the bytes actually being committed.
#
# ⚠️ **Except under `git commit --amend`**, where that sentence is not true.
# `git diff --cached` is index-against-HEAD, so amending shows only the delta:
# fold a four-line fix into a 700-line reviewed commit and the re-review sees
# four lines, not the commit's content. git.md rule 21 names amend as the way to
# fold in a review finding, so this is the normal path, not an edge case — the
# re-review is of the fix, and the original review still stands for the rest.
# It is recorded here because a reader would otherwise assume the stronger
# property that the first paragraph appears to promise.
#
# ## What this cannot make true
#
# ⚠️ **That the review was performed by an agent which did not author the
# change.** Nothing in the artifact proves its provenance; a determined author
# can write the JSON itself. This is the same class as non-negotiable 3 and it
# is honest to say so rather than to imply otherwise: what the gate buys is that
# the review is *about these exact bytes*, which is the property that was
# actually missing. Isolation is bought by the harness — `.claude/agents/reviewer.md`
# and its equivalents — not by this script.
#
# ## Argued findings
#
# A blocking finding is resolved by fixing it (the diff changes, the hash
# changes, review re-runs) or by arguing it in `baselines/review.txt`. Doc 21
# §6. The baseline is a file nobody may grow quietly: a growing argued-list
# means a reviewer is being systematically overruled, and one of the two sides
# is systematically wrong.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

REVIEW_DIR="target/review"
BASELINE="baselines/review.txt"

if git diff --cached --quiet; then
  skip "review (nothing staged)"
  finish
fi

require_sha256 || finish
require_python || finish

h="$(git diff --cached | sha256_stdin)" || { fail "could not hash the staged diff"; finish; }
artifact="$REVIEW_DIR/$h.json"

if [[ ! -f "$artifact" ]]; then
  fail "the staged change has no review"
  note "staged-diff sha256: $h"
  note "run: scripts/review.sh context --task <ID>   then have a reviewer that"
  note "     did not write this change return a verdict to review.sh record"
  finish
fi

# Read the artifact rather than trusting the filename: the directory is
# gitignored and writable, so the name proves nothing on its own.
# ⚠️ Captured into a variable rather than read from a process substitution. A
# `python3` that exists but cannot execute — a fresh macOS ships one that errors
# with `xcrun: no developer tools` until the command-line tools are installed —
# produces no stdout, `read` hits EOF and returns 1, and `set -e` kills the
# script before any FAIL, remedy, or summary prints. The operator's commit is
# refused by a message naming neither the gate nor the fix. The try/except
# inside the interpreter cannot cover the interpreter failing to start.
parsed=""
parsed="$(
  python3 - "$artifact" "$h" <<'PYEOF'
import json, sys
path, expected = sys.argv[1], sys.argv[2]
# Every failure below prints a sentinel and exits 0. ⚠️ Letting an exception
# escape would kill the caller under `set -e` from inside a process
# substitution, so the operator would get a Python traceback and none of the
# FAIL line, the remedy, or the summary — the exact message-and-exit-code
# defect this milestone has now hit twice.
try:
    v = json.load(open(path))
except Exception:
    print("PARSE-ERROR - 0 0 0 - -")
    raise SystemExit(0)
# Valid JSON is not necessarily an object: `[]` parses, and `.get` on it raises.
if not isinstance(v, dict):
    print("NOT-AN-OBJECT - 0 0 0 - -")
    raise SystemExit(0)
if v.get("diff_sha256") != expected:
    print("HASH-MISMATCH - 0 0 0 - -")
    raise SystemExit(0)
findings = [f for f in v.get("findings", []) if isinstance(f, dict)]
blocking = [f for f in findings if f.get("severity") == "blocking"]
major = [f for f in findings if f.get("severity") == "major"]
print(v.get("task_id", "-"),
      v.get("verdict", "-"),
      len(findings),
      len(blocking),
      len(major),
      ",".join(str(f.get("id", "MISSING-ID")) for f in blocking) or "-",
      ",".join(str(f.get("id", "MISSING-ID")) for f in major) or "-")
PYEOF
)" || parsed=""

if [[ -z "$parsed" ]]; then
  fail "the review artifact could not be read"
  note "$artifact"
  note "python3 produced no output — check that it runs: python3 -c 'print(1)'"
  finish
fi

read -r task_id verdict n_findings n_blocking n_major blocking_ids major_ids <<< "$parsed"

case "$task_id" in
  PARSE-ERROR)
    fail "review artifact is not valid JSON: $artifact"
    finish
    ;;
  NOT-AN-OBJECT)
    fail "review artifact is JSON but not an object: $artifact"
    finish
    ;;
  HASH-MISMATCH)
    # The file is named for one diff and claims another. Either it was edited
    # by hand or it was copied from a different change.
    fail "review artifact does not claim the diff it is filed under"
    note "$artifact"
    finish
    ;;
esac

# ⚠️ An empty list is announced, not silently tolerated. `known_task_ids` now
# returns cleanly when the backlog has no rows or cannot be read, which is
# right — but treating that as "nothing to check" and still printing `ok` makes
# a check that stopped running look like a check that passed. lib.sh's contract
# is that a gate says what it checked even when it passes, and `skip` exists to
# be loud. check-commit-msg.sh, reading the same helper, already does this.
known="$(known_task_ids)"
if [[ -z "$known" ]]; then
  skip "backlog membership (the backlog lists no task ids)"
# ⚠️ `-F`, and a here-string rather than `printf | grep`. `-F` because
# without it the id is a regular expression, and a hand-written artifact
# whose task_id is `.*` matches every backlog row — satisfying this check
# while naming no real task. The here-string because `grep -qxF` exits at
# the first match, `printf`'s remaining write can then SIGPIPE, and
# `pipefail` reports that early exit as failure — misreporting a `task_id`
# that genuinely exists as unknown. `M-1.38`: this is the sibling site
# `check-milestone-review.sh` already fixed for both reasons; this one was
# missed the first time. Reproduced before fixing: a ~20,000-row `known`
# list with the real match on line 1 makes the old `printf | grep -qx` form
# report "not found" for a task id that is, in fact, present. See
# `portability.md`'s "Shell scripting" section (rules 21-22) for the general
# idiom this is one instance of.
elif ! grep -qxF "$task_id" <<< "$known"; then
  fail "review names $task_id, which the backlog does not list"
  finish
fi

# The argued list is read **from the index**, never from the working tree.
#
# ⚠️ Reading the working-tree file made the trace-free path the rewarded one. An
# author could append a line to `baselines/review.txt` without staging it: the
# staged diff is unchanged, so the recorded verdict stays valid, the grep
# matches, the gate passes — and the commit records the pristine template. The
# milestone-boundary reader the file's own header demands sees an empty
# argued-list while blocking findings have been quietly suppressed. Staging the
# entry instead moved the hash and invalidated the review, so doing it properly
# was punished.
#
# From the index, arguing a finding is part of the commit. That moves the hash
# and forces one more review round.
#
# ⚠️ That round is not guaranteed to converge, and pretending otherwise would be
# the kind of claim this project keeps catching. The id is sha256(file+summary),
# so it survives unrelated edits — but the next round is a different invocation
# looking at a different diff, and a reviewer that words the same defect
# differently produces a different id, leaving the staged argument dead. The
# author can always fix instead, so it is not a livelock; the cost is that the
# baseline can accumulate several ids for one defect, which blunts the single
# signal it exists to carry. Re-derive the id from something stabler than prose
# if that starts happening.
baseline_staged() {
  git show ":$BASELINE" 2>/dev/null || true
}

# Is this finding argued, with a reason, in the staged baseline?
#
# ⚠️ One function rather than the same grep written twice. The second copy lost
# the id validation, and an id of `.*` from a hand-written artifact then turned
# the pattern into one that matches the baseline's own comment lines — arguing
# a finding nobody argued. The directory is gitignored and writable, so a
# hand-written artifact is a threat model this script has already adopted.
argued() {
  local id="$1"
  [[ "$id" =~ ^[0-9a-f]{12}$ ]] || return 2
  baseline_staged | grep -qE "^[[:space:]]*${id}[[:space:]]+[^[:space:]]"
}

if [[ -f "$BASELINE" ]] && ! git diff --quiet -- "$BASELINE" 2>/dev/null; then
  warn "$BASELINE has unstaged edits, which are ignored — stage them to argue a finding"
fi

# Every blocking finding is fixed or argued. Nothing else counts as resolved.
unresolved=0
if [[ "$blocking_ids" != "-" ]]; then
  IFS=',' read -ra ids <<< "$blocking_ids"
  for id in "${ids[@]}"; do
    # ⚠️ The id is checked before it reaches a regex. An artifact written by
    # hand can omit it, and a `?` interpolated into the pattern below turns
    # `[[:space:]]*?[[:space:]]` into something that matches any indented line
    # in the baseline — silently arguing a finding nobody argued.
    # A reason is required, not optional. Doc 21 §6 says the entry names the
    # finding *and* why it is not a defect; an id alone suppresses a blocking
    # finding while recording nothing about why, which is precisely the quiet
    # growth the baseline's own header forbids.
    # ⚠️ `|| a=$?`, never `argued "$id"; a=$?`. `set -e` fires on a simple
    # command in an untested context, so the second form killed the script the
    # moment a finding was *not* argued — no FAIL, no remedy, no summary, exit 2
    # for an unusable id. The failure path of the check was the one path that
    # could not report. Third time this milestone has produced this exact bug;
    # the tell is a bare call whose non-zero return is meaningful.
    a=0; argued "$id" || a=$?
    if (( a == 2 )); then
      fail "blocking finding has no usable id ('$id'); re-record with scripts/review.sh"
      unresolved=$((unresolved + 1))
    elif (( a == 0 )); then
      warn "blocking finding $id is argued in $BASELINE, not fixed"
    else
      fail "blocking finding $id is unresolved"
      note "fix it, or stage '$id  <why it is not a defect>' in $BASELINE"
      unresolved=$((unresolved + 1))
    fi
  done
fi

if (( unresolved > 0 )); then
  finish
fi

# Major findings do not block, but they must not vanish either: they live in a
# gitignored artifact, so this is the only moment anyone sees them.
if (( n_major > 0 )); then
  warn "$n_major major finding(s) recorded — see $artifact"
fi

# ⚠️ The verdict is acted on, not merely printed. `record` accepts
# `changes-requested` for a major-only review; if the gate then passed with an
# `ok`, the commit would land and the findings would exist only in a gitignored
# directory that `cargo clean` reclaims — nothing in history would record that
# a reviewer asked for changes. A reviewer who judges major findings
# non-blocking says so by returning `pass`, which still warns above.
# A reviewer who asked for changes must get them. The escape is the same one
# blocking findings have -- argue it in the staged baseline -- so `major` is not
# a severity that quietly evaporates.
if [[ "$verdict" == "changes-requested" ]]; then
  unargued=0
  if [[ "$major_ids" != "-" ]]; then
    IFS=',' read -ra mids <<< "$major_ids"
    for id in "${mids[@]}"; do
      if ! argued "$id"; then
        note "major finding $id is not argued"
        unargued=$((unargued + 1))
      fi
    done
  fi
  if (( unargued > 0 )); then
    fail "the reviewer requested changes, and $unargued finding(s) are neither fixed nor argued"
    note "fix them (the hash moves and review re-runs), or stage an entry in $BASELINE"
    note "$artifact"
    finish
  fi
  warn "verdict is changes-requested, but every finding is argued in $BASELINE"
fi

ok "reviewed for $task_id ($n_findings finding(s), $n_blocking blocking, $n_major major, verdict $verdict)"
note "$artifact"
finish
