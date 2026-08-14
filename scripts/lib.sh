#!/usr/bin/env bash
# Shared plumbing for every gate. Sourced, never executed.
#
# The contract every gate keeps:
#
#   - it exits 0 when the rule holds and non-zero when it does not
#   - a *missing tool* is a skip with a named remedy, never a failure, because
#     a missing tool must not be indistinguishable from a failing check
#   - it says what it checked even when it passes, so a gate that silently
#     stopped running is visible
#
# See docs/internal/standards/portability.md rule 10.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export REPO_ROOT

# Colour only when a human is watching. CI logs and pipes stay clean.
if [[ -t 1 ]]; then
  _RED=$'\033[31m'; _GRN=$'\033[32m'; _YLW=$'\033[33m'; _DIM=$'\033[2m'; _OFF=$'\033[0m'
else
  _RED=''; _GRN=''; _YLW=''; _DIM=''; _OFF=''
fi

_FAILURES=0
_CHECKS=0

# A rule held.
ok() {
  _CHECKS=$((_CHECKS + 1))
  printf '%s  ok %s %s\n' "$_GRN" "$_OFF" "$1"
}

# A rule did not hold. Records the failure; the caller keeps going so one run
# reports every violation rather than only the first.
fail() {
  _CHECKS=$((_CHECKS + 1))
  _FAILURES=$((_FAILURES + 1))
  printf '%sFAIL%s %s\n' "$_RED" "$_OFF" "$1" >&2
}

# Something the reader should know that is not a failure.
warn() {
  printf '%swarn%s %s\n' "$_YLW" "$_OFF" "$1" >&2
}

# The check could not run. Not a failure, and deliberately loud enough to
# notice: a gate that skips forever is a gate nobody is running.
skip() {
  printf '%sskip%s %s\n' "$_DIM" "$_OFF" "$1"
}

# Detail under a result.
note() {
  printf '     %s%s%s\n' "$_DIM" "$1" "$_OFF"
}

# Exit with the verdict. Every gate ends by calling this.
finish() {
  if (( _FAILURES > 0 )); then
    printf '\n%s%d of %d checks failed%s\n' "$_RED" "$_FAILURES" "$_CHECKS" "$_OFF" >&2
    exit 1
  fi
  exit 0
}

# Is there Rust to check yet? Gates that inspect crates call this and skip
# cleanly during the bootstrap, when the workspace does not exist.
has_rust() {
  [[ -f "$REPO_ROOT/Cargo.toml" ]]
}

# require_tool <name> [remedy]
#
# Returns 1 and skips when the tool is absent, so the caller writes:
#
#   require_tool protoc "apt-get install protobuf-compiler" || finish
#
# The remedy is not decoration. A gate that skips without saying how to fix it
# produces a checkout that is quietly less checked than the author believes.
require_tool() {
  local tool="$1" remedy="${2:-}"
  if command -v "$tool" >/dev/null 2>&1; then
    return 0
  fi
  if [[ -n "$remedy" ]]; then
    skip "$tool not installed  (install: $remedy)"
  else
    skip "$tool not installed"
  fi
  return 1
}

# sha256 of stdin, as a bare hex digest.
#
# macOS ships `shasum` and not `sha256sum`, and portability.md rule 2 makes
# macOS a first-class development platform — so a script that hard-codes
# `sha256sum` works for half the people who are supposed to be able to run it.
sha256_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 | cut -d' ' -f1
  else
    return 3
  fi
}

# ⚠️ Deliberately a `fail`, not the `skip` that `require_tool` gives every other
# tool. The convention exists so a missing tool is distinguishable from a
# failing check — but a gate built *on* a hash cannot skip the hash and still
# mean anything, and skipping would silently disable the rule it enforces. With
# two fallbacks this is close to unreachable; if it fires, it is a real problem.
require_sha256() {
  if command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1; then
    return 0
  fi
  fail "no sha256 tool found"
  note "install coreutils (sha256sum), or use a system with shasum"
  return 1
}

# ⚠️ The same departure as require_sha256, for the same reason. A gate whose
# entire parsing and validation logic is Python cannot skip Python and still
# mean anything — it would report success while enforcing nothing. `skip` stays
# the default for genuinely optional tools; this is not one of them.
require_python() {
  if command -v python3 >/dev/null 2>&1; then
    return 0
  fi
  fail "python3 not found"
  note "install: apt-get install python3, or the equivalent for this system"
  return 1
}

# Every task ID this repository knows about, one per line, from the backlog.
# The backlog is the single source; a gate that keeps its own list drifts.
known_task_ids() {
  local backlog="$REPO_ROOT/docs/internal/product/backlog.md"
  [[ -f "$backlog" ]] || return 0
  # ⚠️ `|| true`, because this is a pipeline and every caller writes
  # `known="$(known_task_ids)"`. Under `set -e` + `pipefail` a backlog whose
  # table has no rows makes `grep` return 1, the assignment fails, the caller
  # dies with no output at all — and the `[[ -n "$known" ]]` guard written for
  # exactly that case is never reached. A *missing* backlog returned 0 and was
  # handled gracefully, so the two empty states behaved oppositely.
  grep -oE '^\| (M-?[0-9]+\.[0-9]+) \|' "$backlog" | tr -d '|' | tr -d ' ' || true
}

# Task IDs whose backlog row is not yet `done`.
#
# ⚠️ A finding is discharged by a row something will still act on: `next-task`
# reads `todo`, so citing a row already closed parks the finding where nothing
# will look again. This is checked when a verdict is *recorded* — while the
# author is there to pick a different row — and deliberately not by the gate,
# which would then turn finishing the task into a permanent failure.
open_task_ids() {
  local backlog="$REPO_ROOT/docs/internal/product/backlog.md"
  [[ -f "$backlog" ]] || return 0
  grep -E '^\| M-?[0-9]+\.[0-9]+ \|' "$backlog" \
    | grep -vE '\|[[:space:]]*done[[:space:]]*\|[[:space:]]*$' \
    | grep -oE '^\| (M-?[0-9]+\.[0-9]+) \|' | tr -d '|' | tr -d ' ' || true
}

# The milestone HEAD is working in: the most recent commit whose subject names a
# task, and the milestone that task belongs to.
#
# ⚠️ Not the backlog's first heading, which is what this was. backlog.md says
# "Completed tasks stay here with their commit reference", so a finished
# milestone keeps its section and `grep -m1` returns M-1 forever — from M0
# onward the outer loop would keep checking a fully covered M-1 and report
# success while the milestone actually being built went unread. Reading the
# *last* heading instead would only trade that for a dependency on an unwritten
# ordering convention. History needs no convention: the outer loop's subject is
# commits, so deriving from commits is the coherent choice, and it flips to M0
# exactly when M0's first commit lands — not before, which is what lets a
# milestone's completion still be checked after its last commit.
#
# A subject naming no task (`Revert "…"`, a merge) is skipped rather than ending
# the search, since check-commit-msg.sh exempts those shapes.
current_milestone() {
  local id
  id="$(git log --pretty=%s | grep -m1 -oE '^M-?[0-9]+\.[0-9]+' || true)"
  printf '%s' "${id%%.*}"
}

# Commits whose subject names a task in this milestone, oldest first, minus the
# ones that only record a milestone review.
#
# ⚠️ The `\.` matters: without it `M-1` also matches `M-10.3`. And this lives
# here rather than in each caller because the driver and the gate must agree
# exactly — if the gate enumerates a commit the driver never showed the
# reviewer, `record` refuses the SHA the gate demands and the gate cannot be
# passed at all. Two copies of a definition that must not differ.
#
# ⚠️ **The `reviews/` exclusion is what stops an infinite regress.** Recording a
# milestone review produces a commit, that commit names a task in the milestone,
# and it is therefore uncovered — so covering it needs another review, which
# needs another commit. Measured: a milestone reviewed in full went from 0
# uncovered to 1 the moment its own verdict was committed, permanently. A commit
# whose every changed path is a verdict file is bookkeeping about the review,
# not subject matter for it. ⚠️ Matched by artifact name, not by directory: the
# regress is caused only by the verdicts, and excluding all of `reviews/` also
# dropped a commit that only rewrote `reviews/README.md` — real work, silently
# unreviewed, with the gate reporting full coverage without it. A commit that
# touches a verdict *and* anything else is still enumerated.
milestone_commits() {
  local ms="$1" c paths
  while read -r c; do
    [[ -n "$c" ]] || continue
    paths="$(git show --pretty=format: --name-only "$c" | grep -v '^$' || true)"
    # ⚠️ A here-string, not `printf | grep`. `grep -qv` exits at the first
    # non-matching line, the left side then dies of SIGPIPE, `pipefail` calls
    # that a failure, and `!` inverts it to true — so a commit whose path list
    # overflows the pipe buffer was classified as review bookkeeping and dropped
    # from the milestone entirely. Measured: correct at 1000 changed files,
    # wrong at 1500 (~57 KB). It is exactly the largest commits — generated
    # code, an imported corpus, a mass rename — that would have vanished, and
    # the gate would have reported full coverage without them. Same class as the
    # baseline read in check-milestone-review.sh, found in the code written to
    # fix a different defect in this same change.
    if [[ -n "$paths" ]] && ! grep -qvE '^reviews/milestone-.*\.json$' <<< "$paths"; then
      continue
    fi
    printf '%s\n' "$c"
  done < <(git log --reverse --format='%H %s' \
    | grep -E "^[0-9a-f]+ ${ms//./\\.}\.[0-9]+[,:]" \
    | cut -d' ' -f1 || true)
}
