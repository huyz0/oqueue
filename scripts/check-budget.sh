#!/usr/bin/env bash
# The pre-commit suite's wall clock, against a constant. `M0.16`, NFR-56.
#
#   scripts/check-budget.sh
#
# ## How it measures without doubling what it measures
#
# ⚠️ It does **not** run the suite. `lib.sh` records each gate's own wall clock
# as that gate exits, and this one adds up the entries belonging to the current
# run. A budget gate that re-ran the suite would double the thing it exists to
# keep short, which is a strange shape for a gate about duration.
#
# The grouping key is the **process group id**. `pre-commit` spawns every hook
# with a different `PPID` and the same `PGID`, so `PGID` identifies one run of
# the suite. ⚠️ Measured before it was relied on, not assumed — an earlier
# design keyed on `PPID` and would have seen one gate per group.
#
# ## What the artifact is for
#
# Two files, and the split is load-bearing:
#
#   target/timings/gates.tsv     scratch. Per-gate rows for runs in flight.
#                                ⚠️ **This gate consumes its own group's rows.**
#   target/timings/suite.tsv     the trend. One row per completed suite run.
#
# ⚠️ **Consuming is what makes a second run in the same process group correct.**
# A non-interactive shell puts every descendant in one group, and CI runs
# `pre-commit run --all-files` once per commit of a push inside a single step —
# so without consumption the fifth commit of a push sums five suites and fails a
# 2.3 s suite against a 10 s budget. Reproduced by review before it was fixed.
#
# `suite.tsv` is what makes erosion a **trend rather than one sudden failure**.
# Both are under `target/`, so gitignored and machine-local; a CI runner's copy
# dies with the runner, which is a real limit on the trend and is stated rather
# than implied.
#
# ## ⚠️ What this number is, and is not
#
# It is a **floor**. The suite is measured on a workspace that compiles **no
# async runtime and no cloud SDK** — ADR-0002 chose Tokio without taking the
# dependency, and no M0 task adds one. `M1` adds both, under
# `[profile.dev.package."*"] opt-level = 2`, and the honest expectation is that
# the number moves a lot.
#
# ## ⚠️ What the budget is *of*
#
# **The warm inner loop**, which is what NFR-56 is about: the wait a developer
# absorbs between edit and commit, many times a day. It is **not** the first
# build after a clone or a `cargo clean` — measured here, a cold `target/` puts
# the same suite at **28.6 s**, almost all of it `check-crate.sh` compiling the
# workspace for the first time. Failing that is not detecting erosion, it is
# detecting a build.
#
# The rule: **take out every gate over `COMPILING_GATE_MS`; if what remains is
# under budget, the run was a build.** Erosion is the rest of the suite creeping
# up, and the rest of the suite is exactly what this leaves.
#
# ⚠️ Two weaker rules were tried and rejected, both by measurement:
# `slowest > COMPILING_GATE_MS` alone would have made this gate unfailable from
# `M0.17` onward, since a mutation run is minutes; and requiring the slowest to
# be a strict majority still hid up to the dominator's own duration in erosion —
# 21.6 s of build plus 19.9 s of eroded suite exited 0.
#
# ⚠️ **The residue, stated rather than hidden:** gates *individually* over the
# threshold are never counted, so two gates at 5.1 s each are read as two small
# builds rather than as an eroded suite. That is the price of a single-run
# heuristic. `suite.tsv` marks every exempted run `compiling`, so a trend that is
# permanently `compiling` is itself the signal — and nothing reads that yet.
#
# ⚠️ **The first honest breach is not resolved by raising this literal.**
# Non-negotiable 2 forbids it, and the two legal exits are: make the suite
# faster, or move work to CI and say so. Both are tasks. If neither is possible,
# that is a finding for a milestone review, not an edit to the line below.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# ── The constant ────────────────────────────────────────────────────────────
#
# Measured on 2026-08-16 at commit `2bcf6f6` with `pre-commit run --all-files`
# on a warm cache: **2.27 s wall clock across 14 hooks** — ⚠️ 14 because the
# measurement predates this gate being wired, and the suite it now governs is
# larger. ⚠️ **The current size is deliberately not written here**: `M0.25`
# found three documents holding three different values, and the trend rows this
# gate writes carry the live number. `requirements.md`'s NFR-56 row and
# `roadmap.md` state it, and `m0-complete.sh` asserts both against the config.
# Of the measured 2.27 s, `check-coverage.sh`
# was ~1.0 s and `check-crate.sh` ~0.47 s. ⚠️ Both are in the suite this number
# covers, which `M0.16`'s acceptance requires: a budget measured without them is
# a budget for a suite that no longer exists.
#
# ⚠️ The budget is **10 s**, not 2.3 s. Ten seconds is roughly where a developer
# stops waiting and switches context, so it is the number that means something
# rather than the number that was observed. Setting it at the measurement would
# make the next dependency a gate failure, and the only remedies then are the
# ones non-negotiable 2 forbids.
BUDGET_MS=10000

# How many days of history to keep in the artifact. Trend, not archive.
TIMINGS_KEEP_DAYS=30

# ⚠️ A single gate over this is a *compiling* run, not an eroded suite. Measured:
# warm, the slowest gate is `check-coverage.sh` at ~1.0 s; cold, `check-crate.sh`
# alone is 21.6 s. 5 s sits well clear of the first and well under the second.
COMPILING_GATE_MS=5000

# ⚠️ This gate counts its own elapsed time explicitly below, so it must not also
# leave a row for the next run in the same process group to pick up. Measured:
# without this, a third run in one shell reported more gates than the suite has
# hooks. ⚠️ No count is written here on purpose — `M0.25` found three documents
# holding three different ones, and a fourth copy in a comment is how that
# happened. `m0-complete.sh` asserts the number against the config.
_record_timing() { :; }

# ⚠️ **`OQUEUE_SUPPRESS_TIMING` must not reach this gate.** It exists so a gate
# that shells out to another gate contributes one timing row, and it is set on
# that child call only. Set in *this* environment it would silence every gate,
# leaving nothing to sum — and this script would take its "no timings recorded"
# skip and exit 0, passing an arbitrarily over-budget suite. That is a lever to
# defeat NFR-56 by setting one variable, which review found the moment the
# variable existed.
if [[ -n "${OQUEUE_SUPPRESS_TIMING:-}" ]]; then
  fail "OQUEUE_SUPPRESS_TIMING is set in this gate's environment"
  note "it is for a gate invoking another gate, never for the suite"
  note "unset it: the budget cannot be measured with timing suppressed"
  finish
fi

require_tool ps "install procps, or the equivalent for this system" || finish

pgid="$(ps -o pgid= -p $$ 2>/dev/null | tr -d ' ')"
if [[ -z "$pgid" ]]; then
  skip "suite budget (cannot determine the process group)"
  finish
fi

if [[ ! -f "$_TIMINGS_FILE" ]]; then
  # ⚠️ A skip, not a pass: run standalone before any gate has recorded
  # anything, there is nothing to add up and saying "ok" would be a lie.
  skip "suite budget (no timings recorded yet — run the suite first)"
  note "the artifact is written by every gate's finish(), at $_TIMINGS_FILE"
  finish
fi

# Sum this run's gates, and count them.
read -r total_ms gates <<< "$(
  awk -F'\t' -v pg="$pgid" '$1 == pg { s += $3; n += 1 } END { print s + 0, n + 0 }' \
    "$_TIMINGS_FILE"
)"

if (( gates == 0 )); then
  skip "suite budget (no gate in this run recorded a timing)"
  note "this gate measures a pre-commit run; standalone it has nothing to add up"
  finish
fi

# ⚠️ This gate's own cost is not in the file yet — it records on exit, after
# this check. Counting it now keeps the reported total honest rather than
# systematically one gate short.
own_ms=$(( $(_now_ms) - _GATE_STARTED_MS ))
(( own_ms < 0 )) && own_ms=0
total_ms=$(( total_ms + own_ms ))
gates=$(( gates + 1 ))

# ⚠️ Captured **before** the rows are consumed below, so a failing run can still
# say which gate cost what.
slowest="$(awk -F'\t' -v pg="$pgid" '$1 == pg { printf "  %-28s %6d ms\n", $2, $3 }' \
  "$_TIMINGS_FILE" | sort -k2 -rn -s)"
read -r slowest_ms slowest_name <<< "$(
  awk -F'\t' -v pg="$pgid" '$1 == pg && $3 > m { m = $3; n = $2 } END { print m + 0, n }' \
    "$_TIMINGS_FILE"
)"
# ⚠️ **All** compile-bound gates, not just the slowest. This suite has two
# independent from-scratch cargo builds — `check-crate.sh` into `target/debug`
# and `check-coverage.sh` into `target/llvm-cov-target` — so a rule that
# tolerates one of them fails a genuine cold clone.
read -r compiling_ms compiling_n <<< "$(
  awk -F'\t' -v pg="$pgid" -v t="$COMPILING_GATE_MS" \
    '$1 == pg && $3 > t { s += $3; n += 1 } END { print s + 0, n + 0 }' "$_TIMINGS_FILE"
)"
warm_ms=$(( total_ms - compiling_ms ))
[[ -z "$slowest_name" ]] && slowest_name="(none)"

# ⚠️ **Consume this group's rows.** Anything belonging to another group is left
# alone — a parallel checkout mid-suite must not lose its timings.
awk -F'\t' -v pg="$pgid" '$1 != pg' "$_TIMINGS_FILE" > "$_TIMINGS_FILE.tmp" 2>/dev/null &&
  mv "$_TIMINGS_FILE.tmp" "$_TIMINGS_FILE" 2>/dev/null || rm -f "$_TIMINGS_FILE.tmp"

# ⚠️ Age out `gates.tsv` too. Rows belonging to a group that never reached this
# gate — a suite killed halfway, a single hook run on its own — are otherwise
# immortal, and the next full suite in that same process group adds them to its
# own total. Same guard and same reason as the trend prune below.
if cutoff="$(date -u -d "-1 days" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null)"; then
  awk -F'\t' -v c="$cutoff" '$4 >= c' "$_TIMINGS_FILE" > "$_TIMINGS_FILE.tmp" 2>/dev/null &&
    mv "$_TIMINGS_FILE.tmp" "$_TIMINGS_FILE" 2>/dev/null || rm -f "$_TIMINGS_FILE.tmp"
fi

# Append to the trend, then prune it by age. Best effort; a failure here must
# not fail the gate.
suite_file="$(dirname "$_TIMINGS_FILE")/suite.tsv"
# ⚠️ **One row per run, written once the verdict is known.** An earlier version
# appended `ok` before the budget was checked, so a failing run was recorded as
# clean and an exempted run got two rows — which defeated the fourth column's
# whole purpose. Found by review.
_trend() {
  printf '%s\t%s\t%s\t%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$total_ms" "$gates" "$1" \
    >> "$suite_file" 2>/dev/null || true
}
if cutoff="$(date -u -d "-${TIMINGS_KEEP_DAYS} days" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null)"; then
  # ⚠️ GNU-only `date -d`, guarded: on BSD the prune is skipped and the trend
  # simply grows. A trend file that grows is worse than one that is pruned and
  # far better than a gate that fails on macOS.
  awk -F'\t' -v c="$cutoff" '$1 >= c' "$suite_file" > "$suite_file.tmp" 2>/dev/null &&
    mv "$suite_file.tmp" "$suite_file" 2>/dev/null || rm -f "$suite_file.tmp"
fi

if (( total_ms > BUDGET_MS )); then
  # ⚠️ What remains after every compile-bound gate is removed. Measured:
  #
  #   21 600 + three small           warm    900 ms  build            -> exempt
  #   22 000 + 21 600 + 1 000        warm  1 000 ms  two cold builds  -> exempt
  #  180 000 + erosion to 60 004     warm 60 004 ms  erosion          -> fail
  #   21 600 + 19 904                warm 19 904 ms  erosion          -> fail
  #    4 200 + 4 100 + 3 900         warm 12 200 ms  erosion          -> fail
  if (( compiling_n > 0 && warm_ms <= BUDGET_MS )); then
    # ⚠️ Reported, recorded in the trend, and not failed. A cold clone or a
    # `cargo clean` must not block the next commit, and CI checks out fresh.
    _trend "compiling:${slowest_name}"
    skip "suite budget (${total_ms} ms counted — a compiling run, not measured against the budget)"
    note "${compiling_n} gate(s) over ${COMPILING_GATE_MS} ms account for ${compiling_ms} ms; the rest is ${warm_ms} ms, under the ${BUDGET_MS} ms budget"
    note "slowest was ${slowest_name} at ${slowest_ms} ms; marked 'compiling' in the trend"
    finish
  fi
  _trend "over"
  fail "the pre-commit suite took ${total_ms} ms, over the ${BUDGET_MS} ms budget (${gates} gates)"
  note "⚠️ do not raise the budget — non-negotiable 2. Make the suite faster,"
  note "   or move work to CI and record that decision."
  note "slowest gates in this run:"
  # ⚠️ No `producer | head` — `portability.md` rules 21-22: `head` exiting first
  # sends SIGPIPE upstream, and under `set -o pipefail` that aborts the script
  # before `finish` runs.
  printf '%s\n' "${slowest}" | awk 'NR <= 5' >&2
  finish
fi

# ⚠️ **This is the sum of each gate's own elapsed time, not the suite's wall
# clock**, and the two differ: pre-commit's own dispatch is not counted, and a
# hook that does not source `lib.sh` counts as zero. Measured here at 2.06 s
# counted against 2.37 s wall. NFR-56 is written as wall clock, so the counted
# figure is an **under**-estimate — which is the safe direction for a budget but
# is worth knowing before `M0.17` adds a hook that may not source `lib.sh`.
_trend "ok"
ok "suite budget (${total_ms} ms counted of ${BUDGET_MS} ms, ${gates} gates)"
finish
