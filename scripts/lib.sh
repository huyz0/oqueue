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

# Every task ID this repository knows about, one per line, from the backlog.
# The backlog is the single source; a gate that keeps its own list drifts.
known_task_ids() {
  local backlog="$REPO_ROOT/docs/internal/product/backlog.md"
  [[ -f "$backlog" ]] || return 0
  grep -oE '^\| (M-?[0-9]+\.[0-9]+) \|' "$backlog" | tr -d '|' | tr -d ' '
}
