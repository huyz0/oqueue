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
