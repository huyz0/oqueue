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

# ── Timings ─────────────────────────────────────────────────────────────────
#
# ⚠️ **Every gate records its own wall clock, and `check-budget.sh` adds them
# up.** The alternative — a budget gate that runs the suite itself — would
# double the suite it is measuring, which is a strange thing for a gate about
# suite duration to do. `M0.16`.
#
# The grouping key is the **process group**, not the parent pid: `pre-commit`
# spawns each hook with a different `PPID` but the same `PGID`, so `PGID` is
# what identifies one run of the suite. Measured, not assumed.
# ⚠️ **Portably.** `date +%s%N` is a GNU extension: BSD `date` emits a literal
# `N`, and `portability.md` rule 2 makes macOS a first-class development
# platform. An earlier version used it unguarded, and the arithmetic below then
# aborted every gate under `set -e` *after* it had printed `ok` — inverting this
# file's own rule that a missing tool is a skip, never a failure. Found by
# review. Order: bash 5's builtin, then GNU `date`, then second resolution.
_now_ms() {
  local raw
  if [[ -n "${EPOCHREALTIME:-}" ]]; then
    # e.g. 1786819635.123456 -> milliseconds, no external command at all.
    # ⚠️ `[.,]`, not `.` — bash formats EPOCHREALTIME with the LC_NUMERIC
    # decimal point, so a comma-radix locale (de_DE, fr_FR, ...) yields
    # `1786820364,006124`. Stripping only `.` left the comma in place and the
    # arithmetic below silently returned a two-digit number, making the budget
    # gate vacuous on those locales. Found by review, reproduced under de_DE.
    raw="${EPOCHREALTIME//[.,]/}"
    printf '%s\n' "$(( 10#${raw:0:16} / 1000 ))"
    return 0
  fi
  raw="$(date +%s%N 2>/dev/null)"
  if [[ "$raw" =~ ^[0-9]{16,}$ ]]; then
    printf '%s\n' "$(( 10#$raw / 1000000 ))"
    return 0
  fi
  raw="$(date +%s 2>/dev/null)"
  if [[ "$raw" =~ ^[0-9]+$ ]]; then
    printf '%s\n' "$(( 10#$raw * 1000 ))"
    return 0
  fi
  printf '\n'   # unusable clock: callers treat empty as "do not record"
}

_GATE_STARTED_MS="$(_now_ms)"
_TIMINGS_FILE="$REPO_ROOT/target/timings/gates.tsv"

_record_timing() {
  # ⚠️ **Only under `pre-commit`.** Every script sourcing this file calls
  # `finish()` — including `review.sh`, `milestone-review.sh` and the negative
  # suite — and `check-budget.sh` groups by the same run key this does, so a
  # `review.sh && git commit` in one shell would charge review time to the
  # pre-commit suite. `PRE_COMMIT=1` is set by pre-commit and by nothing else.
  [[ -n "${PRE_COMMIT:-}" ]] || return 0
  # ⚠️ A gate that shells out to another gate sets this, so one hook contributes
  # one row. Without it `check-mutants.sh` -> `mutants.sh` recorded twice and
  # `check-budget.sh` counted a phantom gate.
  [[ -z "${OQUEUE_SUPPRESS_TIMING:-}" ]] || return 0
  local now ms pgid
  now="$(_now_ms)"
  [[ -n "$now" && -n "$_GATE_STARTED_MS" ]] || return 0
  ms=$(( now - _GATE_STARTED_MS ))
  (( ms < 0 )) && return 0
  # ⚠️ **`OQUEUE_RUN_ID` first, and a container is why.** The pgid identifies
  # one suite run because a shell gives each pipeline its own process group —
  # true natively, false inside a PID namespace, where every run is pgid 1. So
  # a contained run's rows collide with the previous contained run's in a
  # `target/timings` that outlives the container, and `check-budget.sh` sums
  # two runs into one. `scripts/docker-test.sh` sets this per invocation;
  # unset, nothing changes. `M10.24`.
  if [[ -n "${OQUEUE_RUN_ID:-}" ]]; then
    pgid="$OQUEUE_RUN_ID"
  else
    pgid="$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' ')" || pgid=""
  fi
  [[ -n "$pgid" ]] || return 0
  mkdir -p "$(dirname "$_TIMINGS_FILE")" 2>/dev/null || return 0
  # ⚠️ Best effort, and silent on failure. A gate must never fail because it
  # could not write a timing — that would make an observability feature into a
  # source of red builds.
  printf '%s\t%s\t%s\t%s\n' \
    "$pgid" "$(basename "${BASH_SOURCE[-1]}")" "$ms" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    >> "$_TIMINGS_FILE" 2>/dev/null || true
}

# Exit with the verdict. Every gate ends by calling this.
finish() {
  _record_timing
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

# Every non-test source file of the five group-protocol handlers.
#
# ⚠️ **The five handlers are not five files, and two gates have assumed they
# are.** `code-structure.md` rule 16 splits a file at 500 lines, so
# `join_group` is `mod.rs`, `deadline.rs` and five more under `round/`;
# `sync_group` gained `barrier.rs` when `M4.43` gave the barrier a failure
# channel; `heartbeat` has its own `deadline.rs`. A gate naming
# `join_group/mod.rs` reads one of seven. `M4.32`'s own subject one register
# over: a list written the day a gate was, checking the past.
#
# ⚠️ **Tests are excluded by path**, the same scoping
# `check-fencing-seam.sh` argues for itself: a test asserting on an error
# code is reading an answer, not producing one. ⚠️ **By path only, which
# does not reach an inline `#[cfg(test)] mod tests`** — two of the fourteen
# carry one (`join_group/deadline.rs`, `heartbeat/deadline.rs`), so a unit
# test written there that names a code it is asserting about would be read
# as producing it. Found by review; no such test exists today, and the
# repair is to move it to a `tests.rs` beside the module, which
# `code-structure.md` rule 8 already prefers.
group_handler_files() {
  # ⚠️ **`$REPO_ROOT`, not a cwd-relative path.** The helpers either side of
  # this one anchor there; sourced from anywhere else the relative form
  # returned nothing and exited 0, which is a silent empty answer to a
  # question about coverage. Found by review of `M4.48`.
  local root="$REPO_ROOT/crates/oqueue-broker/src"
  local handler found listing out="" missing="" unwalkable=""
  for handler in find_coordinator join_group sync_group heartbeat leave_group; do
    found=0
    if [[ -f "$root/$handler.rs" ]]; then
      out+="crates/oqueue-broker/src/$handler.rs"$'\n'
      found=1
    fi
    if [[ -d "$root/$handler" ]]; then
      # ⚠️ **`find`'s status is taken and its stderr goes nowhere**, which is
      # what makes the contract below ("silent unless it returns non-zero")
      # true rather than merely intended. Both callers capture this helper
      # with `2>&1`, so a diagnostic escaping on the *success* path becomes
      # a file operand in their list. Measured by review of `M4.50`:
      # `chmod 000` on a subdirectory made `find` print one permission error,
      # which sorted ahead of the real paths, and `check-fencing-seam.sh`'s
      # `awk` then died on it having scanned no files — a process-substitution
      # failure escapes `set -e` and `pipefail` both, so that leg printed
      # `ok ... (15 file(s) read)` with a fatal code planted. A directory the
      # walk cannot read is a refusal, not a shorter answer.
      if listing="$(find "$root/$handler" -name '*.rs' -type f \
          -not -name 'tests.rs' -not -path '*/tests/*' 2>/dev/null)"; then
        while IFS= read -r file; do
          [[ -n "$file" ]] || continue
          out+="crates/oqueue-broker/src/${file#"$root/"}"$'\n'
          found=1
        done < <(printf '%s\n' "$listing" | sort)
      else
        unwalkable+=" $handler"
      fi
    fi
    (( found == 0 )) && missing+=" $handler"
  done

  # ⚠️ **Nothing is printed until every handler has answered**, so a failure
  # emits *no* list rather than a partial one. Review of `M4.50` found the
  # first version printing the handlers it had found and then returning 1 —
  # and `mapfile -t f < <(group_handler_files)` cannot see a status, so a
  # caller written against the documented contract would have taken a
  # thirteen-file list as complete. Four places said "refuses to answer at
  # all"; now it does. ⚠️ **And nothing is written to stderr unless this
  # returns non-zero**, which is the half the callers' `2>&1` rests on.
  if [[ -n "$missing" || -n "$unwalkable" ]]; then
    if [[ -n "$missing" ]]; then
      printf 'group_handler_files: no source file for handler(s):%s\n' "$missing" >&2
    fi
    if [[ -n "$unwalkable" ]]; then
      printf 'group_handler_files: could not walk handler(s):%s\n' "$unwalkable" >&2
    fi
    return 1
  fi

  # ⚠️ **Each handler contributing a file is the invariant, replacing a count
  # of the total.** `M4.48` floored the total at fourteen; a file count can
  # lawfully shrink — folding a small module into its parent is legal under
  # `code-structure.md` rule 16 — and the floor then left only exits
  # non-negotiable 2 forbids. ⚠️ **It is not strictly stronger, and review
  # measured the residue**: moving a whole subtree out (`join_group/round/`
  # to `src/group/round/`) leaves `join_group` still contributing `mod.rs`,
  # so this passes at nine files where the count failed. Catching that needs
  # the module graph rather than the directory tree, which is what a caller
  # should reach for if it ever matters; recorded here rather than implied
  # away.
  printf '%s' "$out"
}

# Runs a command with a wall-clock ceiling, and returns 124 if it hit one.
#
# ⚠️ **Not `timeout(1)`, which stock macOS does not ship** — the same reason
# `sha256_stdin` above exists, and `portability.md` rule 2 makes macOS a
# first-class development platform. GNU coreutils' `timeout` is available
# through Homebrew as `gtimeout` and not at all by default, so a gate built on
# it works for half the people who are supposed to be able to run it.
#
# ⚠️ **A ceiling is not a nicety for a harness that drives real clients.** A
# broker regression that leaves a rebalance round open makes the client library
# wait, and a script whose own deadline fires can still hang at interpreter
# exit inside that library's teardown — measured for `confluent-kafka` in
# `M4.38`. Without a ceiling the gate stalls instead of failing, which is worse
# than a red run: nobody reads a job that never ends.
#
# `SIGTERM` first so a runtime gets to flush, then `SIGKILL`, with a fixed
# two-second grace between them — so a child that dies at once on `SIGTERM`
# still costs those two seconds, and a ceiling of N returns at about N+2. The
# `wait` on each of the two exit paths is what reaps the child rather than
# leaving a zombie for the caller.
#
# ⚠️ **Three limits `timeout(1)` does not have, measured by review of
# `M4.44`.** ⚠️ **The first of them is reachable now**, and this paragraph
# said the opposite until `M4.53`: it read "none is reachable from this
# file's own call sites, where the wrapped command is a leaf process", which
# was true of the two group scripts `M4.44` wrapped and is what left the
# harness's other client legs unexamined. `M4.53` bounded them, and
# `idempotent_conformance.py`, `tls_sasl.py` and `offset_survival.py` each
# start a broker of their own, so a ceiling that fires leaves it running —
# `kafka-client-harness.sh`'s EXIT trap covers only the broker *it* started.
# `M4.61` is the row. Every gate in `scripts/` sources `lib.sh`, so all three
# limits are written down rather than left to be rediscovered (an earlier
# version of this sentence carried a count, which was wrong and would have
# drifted on the next script added):
#
#   * **Only the direct child is signalled** — by the ceiling, and by the
#     interrupt forwarding below alike. `run_bounded 3 bash -c 'sleep 222 &
#     sleep 100'` returns 124 with `sleep 222` still running. A wrapped shell
#     function behaves the same way. Killing a process group would need
#     `set -m` and a negative pid, which changes job-control behaviour for the
#     whole caller. `M4.61` is the row for the three harness legs where this
#     is reachable.
#   * **Liveness is `kill -0` on a pid this shell has already reaped**, so on
#     pid reuse the loop can time a stranger and then signal it. Low
#     probability, not zero.
#   * **The `wait` after `SIGKILL` is unbounded.** A child wedged in
#     uninterruptible sleep — a hung bind mount under WSL2 is the realistic
#     one — reproduces the stall this function exists to remove, one line
#     further on.
run_bounded() {
  local seconds="$1"
  shift
  "$@" &
  local pid=$!
  # ⚠️ **Ctrl-C did not stop the wrapped command, and `&` is why** — `M4.57`.
  # A script runs with job control off, and bash makes an asynchronous job
  # ignore `SIGINT` there; the terminal delivers the interrupt to the whole
  # foreground process group, so this shell took it and the *client* did not.
  # A developer interrupting a stalled harness leg stopped neither it nor the
  # harness — and killing the harness instead fired its `EXIT` trap on the
  # broker while leaving the client orphaned. Forwarding it here is what
  # restores the ordinary meaning of Ctrl-C without `set -m`, which would
  # change job-control behaviour for every caller of this file.
  #
  # ⚠️ **`INT` re-raises rather than returning a code.** A shell that swallows
  # its own interrupt and carries on is the second half of the same
  # complaint: the trap is removed first so the default disposition applies,
  # and the signal is re-sent to this shell so callers up the stack see a
  # terminated run rather than a gate that failed.
  # ⚠️ **A probe for this must not start the runner as an async job**, and
  # three of them measured their own scaffolding before one measured this.
  # Bash sets an async job's `SIGINT` to `SIG_IGN`, that survives `exec`, and
  # bash then *refuses to install a trap* for a signal ignored at startup —
  # `trap -p INT` prints `trap -- '' SIGINT` and the line below is a no-op.
  # A developer's terminal gives the harness the default disposition, so the
  # probe has to as well: launch it through something that resets the
  # disposition before `exec`, then signal the process group.
  #
  # ⚠️ **Both arms re-raise, and the `TERM` one did not until review caught
  # it.** A handler that stops the child and returns leaves the *caller*
  # running: measured against a harness-shaped caller, `kill` on it killed
  # only the client, `run_bounded` returned 143, the `|| rc=$?` at the call
  # site swallowed it, and the harness went on to start the next leg — so
  # `docker stop` would have been ignored until the daemon's SIGKILL, and
  # 143 is neither 0 nor 124, so every leg branch reports a deliberately
  # terminated run as a conformance failure. Swallowing a signal is the
  # complaint this function exists to answer, one signal over.
  #
  # ⚠️ **The caller's own disposition is restored, not cleared.** `trap` is
  # shell-global rather than function-scoped, so `trap - INT TERM` on return
  # would silently delete a `trap cleanup INT TERM` the caller installed
  # before calling — latent today, since no script in `scripts/` sets one,
  # and invisible when it arrives because the caller's trap line is still
  # there to read.
  local saved_traps
  saved_traps="$(trap -p INT TERM)"
  trap '_run_bounded_stop "$pid"; _run_bounded_restore "$saved_traps"; trap - INT; kill -INT $$' INT
  trap '_run_bounded_stop "$pid"; _run_bounded_restore "$saved_traps"; trap - TERM; kill -TERM $$' TERM
  local waited=0
  while (( waited < seconds )); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 1
    waited=$((waited + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    _run_bounded_stop "$pid"
    wait "$pid" 2>/dev/null || true
    _run_bounded_restore "$saved_traps"
    return 124
  fi
  local rc=0
  wait "$pid" || rc=$?
  # ⚠️ **Restored on every return path**, or the next command in the caller
  # runs under a trap naming a `$pid` this function has already reaped — and
  # on pid reuse that signals a stranger, which is the hazard the second
  # bullet above already records.
  _run_bounded_restore "$saved_traps"
  return "$rc"
}

# `SIGTERM`, a grace period, then `SIGKILL` — the escalation `run_bounded`
# uses both when its ceiling fires and when it forwards an interrupt.
#
# ⚠️ **The interrupt path had no escalation and review caught that too.** It
# sent one `TERM` and then killed its own shell in the same handler, so a
# child that ignores or blocks in a `SIGTERM` handler was not stopped at all
# and nothing survived to escalate — the Java legs are the realistic case,
# where a JVM runs shutdown hooks against a broker the caller's `EXIT` trap
# is killing at that same moment.
#
# ⚠️ **Polls rather than sleeping the whole grace**, so a child that dies at
# once costs milliseconds instead of the two seconds the ceiling path used to
# spend unconditionally.
_run_bounded_stop() {
  local pid="$1" waited=0
  kill -TERM "$pid" 2>/dev/null || true
  while (( waited < 2 )) && kill -0 "$pid" 2>/dev/null; do
    sleep 1
    waited=$((waited + 1))
  done
  kill -KILL "$pid" 2>/dev/null || true
}

# Puts back whatever `INT`/`TERM` disposition the caller had, given the
# output of `trap -p INT TERM` taken before `run_bounded` installed its own.
#
# ⚠️ **Clear first, then re-apply.** An empty capture means the caller had no
# handler, and `eval ""` alone would leave `run_bounded`'s still installed.
_run_bounded_restore() {
  trap - INT TERM
  if [[ -n "$1" ]]; then
    eval "$1"
  fi
  return 0
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

# The backlog's content as it is **staged**, never the working tree. `M-1.39`.
#
# ⚠️ Every other input `check-reviewed.sh` and `check-milestone-review.sh`
# read — the review artifact's hash, the argued baseline — already came from
# the index (`check-reviewed.sh`'s own `baseline_staged()` reads
# `git show ":$BASELINE"` for exactly this reason). Reading the backlog from
# disk instead let an unstaged row satisfy `known_task_ids`/`open_task_ids`
# locally while the same gate failed on CI, which checks out the index —
# the identical pass-here/red-there asymmetry `check-reviewed.sh`'s baseline
# read was already fixed for. Shared by both functions below because they
# read the same file the same way; a second copy of this line is a second
# place to fix if the read ever needs to change again.
#
# ⚠️ `2>/dev/null || true`: `git show` fails loudly — exit 128, a `fatal:`
# line on stderr — when the path is not staged at all: a fresh checkout
# before anything is `git add`ed, or a repository with no `backlog.md` yet.
# That is the same "missing" state the old `[[ -f "$backlog" ]]` guard
# handled gracefully, and it must stay silent here for the same reason: a
# missing backlog is not this function's failure to report.
_backlog_from_index() {
  # ⚠️ **Fenced code blocks are blanked, not stripped**, so every reader agrees
  # about them and line numbers still point at the file. A row quoted inside a
  # ``` block is documentation, not a row — but `check-backlog-rows.sh` skipped
  # those lines while these greps did not, so a fenced example naming a real id
  # made `known_task_ids` emit it twice and `open_task_ids` call a closed row
  # open, with the gate reporting `ok`. That is the two-parsers-disagree failure
  # `M4.27` exists to remove, reproduced inside `M4.27`. Blanking rather than
  # deleting keeps `backlog.md:<n>` in the gate's messages resolvable.
  #
  # ⚠️ **`LC_ALL=C`, and it is the whole point rather than a habit.** Without it
  # `[[:space:]]` follows the ambient locale and matches U+1680, U+2000-U+200A,
  # U+2028/9, U+205F, U+3000 — while the balance check in
  # `check-backlog-rows.sh` matches bytes. Review reproduced the gap twice, each
  # time in the fail-open direction: awk blanked the rest of the file and the
  # counter saw nothing wrong, so every row below went unchecked with the gate
  # reporting `ok`. Pinning the locale makes `[[:space:]]` exactly
  # `[ \t\n\v\f\r]`, which is a definition the other reader can hold too.
  LC_ALL=C git -C "$REPO_ROOT" show ":docs/internal/product/backlog.md" 2>/dev/null \
    | LC_ALL=C awk '/^[[:space:]]*```/ { infence = !infence; print ""; next }
           infence { print ""; next }
           { print }' \
    || true
}

# The shape of a backlog task row's leading cell, and the one definition of it.
#
# ⚠️ **Two readers use it today, in two languages** — the functions below, and
# `check-backlog-rows.sh`, which is handed the pattern rather than writing a
# second copy. ⚠️ **It is not yet every reader, and saying so is the point**:
# `milestone-review.sh` hand-writes a milestone-scoped variant, and
# `check-commit-msg.sh` hand-writes the commit-*subject* form. `TASK_ID_RE`
# exists so those can converge here without a third spelling appearing first;
# until they do, an id-form widening still has to visit them. `M4.27` is why it is a constant: that task
# added a gate whose whole subject is malformed rows, and the first version of
# it carried its own Python transcription of this pattern. Two parsers
# disagreeing about a malformed row is the failure that gate exists to catch,
# reproduced inside the gate.
#
# ⚠️ **`[a-z]?` added by `M10.33`.** `M10.0`'s own three-way split of `M3.42`
# ("one row per gate script, because three scripts is three commits") named
# two of the rows `M10.18a` and `M10.18b` — the established convention this
# backlog already uses for a dissolved row's siblings, `M3.41`/`M3.42`/`M3.43`
# and their own predecessors. Without the suffix here, `M10.18a` matched
# neither function below: `known_task_ids` returned nothing for it,
# `check-reviewed.sh` refused a verdict recorded against a genuinely open,
# correctly-formatted row with "which the backlog does not list", and the row
# could not be closed at all until this line changed — found by trying to
# commit against it, not by inspection. ⚠️ That it had to be widened in three
# places at once is the argument for this constant.
TASK_ID_RE='M-?[0-9]+\.[0-9]+[a-z]?'
TASK_ROW_RE="^\\| ($TASK_ID_RE) \\|"

# The states that mean a row is still *open* — one something will act on.
#
# ⚠️ **`sdd.md`'s `states:open` marker block is the owner, and
# `check-backlog-rows.sh` compares this against it in both directions**, so it
# cannot drift silently the way a second hand-maintained list would.
#
# ⚠️ **Open, not closed, and that direction is the fix.** This read
# `TASK_STATES_CLOSED='done|dissolved'` for one review round, which is a second
# unchecked copy of the vocabulary wearing a comment that claimed otherwise:
# review added a fourth state to `sdd.md`, set a row to it, and `open_task_ids`
# reported that row **open** while the gate said `ok` — the exact
# `dissolved`-looked-open bug `M4.27` fixes, reintroduced one level up. Listing
# what is open makes an unknown state closed by default, which is the safe
# direction: a finding cannot be discharged by a row nobody will work.
TASK_STATES_OPEN='todo'

# Every task ID this repository knows about, one per line, from the backlog.
# The backlog is the single source; a gate that keeps its own list drifts.
#
# ⚠️ **`[a-z]?` added by `M10.33`.** `M10.0`'s own three-way split of `M3.42`
# ("one row per gate script, because three scripts is three commits") named
# two of the rows `M10.18a` and `M10.18b` — the established convention this
# backlog already uses for a dissolved row's siblings, `M3.41`/`M3.42`/`M3.43`
# and their own predecessors. Without the suffix here, `M10.18a` matched
# neither this function nor `open_task_ids` below: `known_task_ids` returned
# nothing for it, `check-reviewed.sh` refused a verdict recorded against a
# genuinely open, correctly-formatted row with "which the backlog does not
# list", and the row could not be closed at all until this line changed —
# found by trying to commit against it, not by inspection.
known_task_ids() {
  local backlog; backlog="$(_backlog_from_index)"
  [[ -n "$backlog" ]] || return 0
  # ⚠️ `|| true`, because this is a pipeline and every caller writes
  # `known="$(known_task_ids)"`. Under `set -e` + `pipefail` a backlog whose
  # table has no rows makes `grep` return 1, the assignment fails, the caller
  # dies with no output at all — and the `[[ -n "$known" ]]` guard written for
  # exactly that case is never reached. A *missing* backlog returned 0 and was
  # handled gracefully, so the two empty states behaved oppositely.
  LC_ALL=C grep -oE "$TASK_ROW_RE" <<< "$backlog" | tr -d '|' | tr -d ' ' || true
}

# Task IDs whose backlog row is one something will still act on.
#
# ⚠️ A finding is discharged by a row something will still act on: `next-task`
# reads `todo`, so citing a row already closed parks the finding where nothing
# will look again. This is checked when a verdict is *recorded* — while the
# author is there to pick a different row — and deliberately not by the gate,
# which would then turn finishing the task into a permanent failure.
#
# ⚠️ **`dissolved` is closed, not open, and `M4.27` is where that was fixed.**
# This read "not `done`" until then, which made a `dissolved` row look open —
# and `sdd.md`'s state list, which `M4.27` wrote, says `dissolved` means "the
# row will never be worked". So a blocking finding filed against a `dissolved`
# row was accepted by `milestone-review.sh record` and parked exactly where
# nothing looks again, which is the one thing this function exists to prevent.
# Found by review, as the concrete instance of two parsers disagreeing about
# what a state cell means.
open_task_ids() {
  local backlog; backlog="$(_backlog_from_index)"
  [[ -n "$backlog" ]] || return 0
  # ⚠️ `LC_ALL=C` here too: `[[:space:]]` around the state cell must mean the
  # same bytes it means to `check-backlog-rows.sh`, or a row whose trailing
  # space is U+00A0 is a valid `todo` row to one reader and closed to the other.
  LC_ALL=C grep -E "$TASK_ROW_RE" <<< "$backlog" \
    | LC_ALL=C grep -E "\|[[:space:]]*($TASK_STATES_OPEN)[[:space:]]*\|[[:space:]]*$" \
    | LC_ALL=C grep -oE "$TASK_ROW_RE" | tr -d '|' | tr -d ' ' || true
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
    | grep -E "$(milestone_subject_re "$ms")" \
    | cut -d' ' -f1 || true)
}

# The one regex deciding "this commit's subject names milestone $1" -- `M2.8`
# (M1.48's minor d): it existed in three byte-similar copies (here and inline
# in check-milestone-review.sh and milestone-review.sh), and every
# operator-facing claim about which commits count is true only while the
# copies agree. Works against `%H` and `%h` alike -- both are `[0-9a-f]+`.
#
# ⚠️ **`[a-z]?` added by the M10 closing review, the fourth copy of `M10.33`'s
# fix.** `known_task_ids`/`open_task_ids` here and `check-commit-msg.sh`'s
# subject pattern were widened for a lettered task id (`M10.18a`, `M10.18b`);
# this one was not, so `M10.18a: ...` and `M10.18b: ...` never matched and
# `milestone_commits` silently excluded both from every milestone they were
# ever in -- a gap the closing review that reads `milestone_commits`' own
# output could not see, because the exclusion happens before that output
# exists.
milestone_subject_re() {
  printf '^[0-9a-f]+ %s\\.[0-9]+[a-z]?[,:]' "${1//./\\.}"
}
