#!/usr/bin/env bash
# The M10 completion condition, as `M10.md` states it and `M10.0`/`M10.7`/
# `M10.9` corrected it: a known-bad build fails under simulation and its seed
# replays to the same failure; the invariant harness runs across a seeded
# schedule with faults and checks continuously, including the NFR-21 leg
# `M10.0` moved here from M3; the simulated clock is the only clock; and the
# write path's enumerated crash points are structurally complete.
#
# ## Not "partitions", which `M10.md`'s sentence says and `M10.0` deferred
#
# The only partition the plan enumerated — the agent-coordinator link —
# went to `M7` with the follower whose cache can go silent
# (`roadmap.md`'s deferral table). A leg here would run against
# `Cluster::cache_state`'s `silent_for_ms` hardcoded to zero and constrain
# nothing. This gate does not assert partitions and says so, rather than
# writing a leg that passes for no reason.
#
# ## Each leg is falsifiable or says it is not
#
# `M3.16` is the worked precedent this row names: two legs there were
# structurally unfalsifiable and a third overclaimed, and the fix was a gate
# that prints what it actually asserts rather than what the milestone's prose
# suggests. Two legs below follow the same discipline:
#
#   - **"pauses and 503s"** is `M10.md`'s phrase for the invariant harness's
#     faults. What the *broker-level* `Schedule` (`M10.12`) actually draws is
#     `StormStore` (a 503) and `RefuseJournal` — no `Pause` step. `Fault::Pause`
#     exists only in `oqueue-store`'s simulated S3 (`M10.8`), one layer below
#     where the invariant harness runs, and `M10.31` is the open row for a
#     per-operation pause the broker-level harness could draw. The leg below
#     names storms and refusals, not pauses, because that is what runs.
#   - **"a known-bad build fails and its seed replays"** is asserted by the
#     synthetic case `ADR-0028` built to prove the property itself
#     (`seed.rs`'s own tests), not by a mutated copy of this workspace — a
#     completion gate that compiled a known-bad tree on every milestone
#     boundary would be its own maintenance burden, and the property under
#     test is about the *harness*, not about any one bug. The leg says so.
#
# ## Where this runs, and why not in pre-commit
#
# Standalone, at the milestone boundary — the four gates before it do the
# same. `NFR-56` gives the whole pre-commit suite 10 s; this runs the
# workspace suite, a multi-thread runtime, and `check-milestone-review.sh`
# besides.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

FLUSH_SEAM="crates/oqueue-broker/src/flush.rs"

# ── 0a. Every M10 commit read as a whole ────────────────────────────────────
# Above every skip and every tool requirement (M1.43's finding, m3-complete.sh's
# precedent): this check needs no cargo, so nothing below may gate it.
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M10 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "every M10 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "scripts/check-milestone-review.sh could not be run (exit 127)"
  note "the gate is absent, not the review -- M10's coverage is unknown, not failing"
else
  fail "M10's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M10 to build the packet"
fi

# ── 0b. M10.9's leg: the write path's crash points are structurally complete ─
#
# ⚠️ **`M10.9`'s own row records that this did not exist rather than implying
# it did.** `Cluster::flush` has two `.await`s, which is the whole basis of
# `crash_points.rs`'s three-window enumeration — nothing in the compiler
# notices a *third* `.await` appearing and adding a fourth window the
# enumeration does not cover. This counts them structurally, above the cargo
# guard, for the same reason `m3-complete.sh`'s zero-LIST check is: a file
# this needs might move or change shape on a machine with no toolchain at
# all, and the gate should say so rather than skip silently.
if [[ ! -f "$FLUSH_SEAM" ]]; then
  fail "$FLUSH_SEAM not found -- the crash-point count is asserted from this file's shape"
else
  await_count="$(grep -c '^\s*\.await$' "$FLUSH_SEAM" || true)"
  if [[ "$await_count" == "2" ]]; then
    ok "Cluster::flush has exactly 2 .await points, matching crash_points.rs's three-window enumeration"
  else
    fail "Cluster::flush has $await_count .await point(s) on its own line, not the 2 crash_points.rs enumerates"
    note "a changed count means a fourth window exists and crates/oqueue-broker/tests/it/crash_points.rs"
    note "does not cover it -- see M10.9's backlog row"
  fi
fi

if ! has_rust; then
  skip "everything below (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish

# run_counted <label> <min-tests> <cargo test args...>
#
# `m2-complete.sh`'s helper, `m3-complete.sh`'s precedent: a cargo test
# filter that matches nothing exits 0 ("0 passed; N filtered out"), so a leg
# named by filter alone goes green the day a refactor renames the test.
run_counted() {
  local label="$1" min="$2"; shift 2
  local out passed
  if ! out="$(cargo test "$@" --quiet 2>&1)"; then
    fail "$label failed"
    note "run: cargo test $*"
    return 1
  fi
  passed="$(printf '%s\n' "$out" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s+0}')"
  if (( passed < min )); then
    fail "$label matched only $passed test(s), floor $min -- a renamed test leaves a filter green"
    note "run: cargo test $*"
    return 1
  fi
  ok "$label ($passed test(s))"
}

# ── 1. The workspace suite ──────────────────────────────────────────────────
if cargo test --workspace --quiet >/dev/null 2>&1; then
  ok "cargo test --workspace"
else
  fail "cargo test --workspace failed"
  note "run it directly for the failure detail"
  finish
fi

# ── 2. A failing seed replays to the same failure, a different seed does not ─
# ⚠️ **The property, not one bug.** `ADR-0028`'s own probe is what this pins:
# a schedule-dependent assertion that fails under one seed, fails identically
# on replay from it, and passes under a seed reaching the other arm.
run_counted "a failing seed replays identically, and a different seed reaches the other outcome" 1 \
  -p oqueue-testkit --lib seed::tests::a_failure_replays_from_its_seed_and_a_different_seed_does_not || finish

# ── 3. The invariant harness, across a seeded schedule, checked continuously ─
# ⚠️ **NFR-21's leg, by name** -- `requirements.md` gives it "invariant test:
# the cache is never ahead of durability", and this is the row `M10.0` moved
# the requirement to. A green m10-complete.sh that does not run this is the
# arrangement M3 was marked complete out of.
run_counted "NFR-21: the cache is never ahead of durability, checked at every step of a faulted run" 1 \
  -p oqueue-broker --test it invariants::the_three_invariants_hold_at_every_step_of_a_faulted_run || finish

# ⚠️ **"pauses and 503s", scoped**: the broker-level `Schedule` draws
# `StormStore` (a 503 storm) and `RefuseJournal`, not a `Pause` step -- see
# this file's own header for why the stronger phrase does not hold here.
run_counted "the invariants hold across a schedule drawn from a seed (503 storms, journal refusals)" 1 \
  -p oqueue-broker --test it generated::a_drawn_schedule_holds_the_invariants || finish
run_counted "a failing drawn schedule reduces to the steps its failure actually needs" 1 \
  -p oqueue-broker --test it generated::shrinking_reduces_a_drawn_schedule_to_steps_that_are_all_necessary || finish

note "the harness has no broker-level Pause step -- Fault::Pause exists only in oqueue-store's"
note "simulated S3 (M10.8); a per-operation pause the invariant harness could draw is M10.31"
note "partitions are not asserted -- the only one M10.md named is M7's (roadmap.md's deferral table)"

# ── 4. The simulated clock is the only clock ────────────────────────────────
# ⚠️ Re-run rather than trusted from pre-commit history: this gate runs
# standalone and should not assume the tree it is handed already passed
# every pre-commit gate.
if bash "$REPO_ROOT/scripts/check-sans-io.sh" >/dev/null 2>&1; then
  ok "the simulated clock is the only clock (check-sans-io.sh's REAL_CLOCK_RE)"
else
  fail "check-sans-io.sh failed -- a real clock read exists where the seeded run cannot control it"
  note "run: scripts/check-sans-io.sh"
fi

# ── 5. The worked example: a known failure class, caught end to end ────────
run_counted "the worked example: a dead coordinator loop is noticed, not served past" 1 \
  -p oqueue --bin oqueue the_accept_loop_notices_a_dead_coordinator_rather_than_serving_past_it || finish

# ── 6. The seed corpus's machinery is sound ─────────────────────────────────
# ⚠️ **`--self-test`, not a replay.** The corpus is empty of failures by
# design (`M10.11`'s row), so what this checks is that the reader refuses a
# malformed row -- an empty corpus proves nothing about itself.
if bash "$REPO_ROOT/scripts/seed-corpus.sh" --self-test >/dev/null 2>&1; then
  ok "the seed corpus's reader refuses a malformed row (--self-test)"
else
  fail "scripts/seed-corpus.sh --self-test failed"
  note "run: scripts/seed-corpus.sh --self-test"
fi

# ── 7. The verdict ───────────────────────────────────────────────────────────
# ⚠️ Three of these legs are scoped narrower than M10.md's sentence reads, and
# the final line refuses to launder that: it names them.
if (( _FAILURES == 0 )); then
  ok "M10 completion condition holds, with 'pauses' scoped to oqueue-store's model, partitions"
  ok "unasserted (M7's), and the seed-replay leg pinning the property rather than one bug"
fi
finish
