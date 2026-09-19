#!/usr/bin/env bash
# The M6 completion condition, as `M6.md` states it: a chaos test kills a
# coordinator under load with no acknowledged record lost and no manual step
# (FR-51, NFR-20); a paused coordinator whose lease expired fences itself
# (task 13); a node with an empty disk serves correctly (NFR-44); a node whose
# coordinator is unreachable at boot comes up degraded (task 14); hot-standby
# failover and cold rebuild are timed as separate numbers (NFR-22, measured not
# bounded); a rebuilt index answers every query as the original did; and
# committed consumer offsets survive a restart (FR-21, inherited from M4).
# Plus leg 0 (the milestone read as a whole) and leg 0b (every open row handed
# on), as `m5-complete.sh` carries them.
#
# ⚠️ **Written first, and red** (`milestone/SKILL.md`, `M5.84`): while a leg
# fails it is the statement of what is left, and the loop that drives M6 stops
# when this exits 0 — never when the backlog runs out.
#
# ⚠️ **Every requirement leg is a named test run with a pass-count floor.** A
# filter that matches nothing exits 0, so the count is the check; and a leg
# whose test does not exist fails rather than skips, because this is an exit
# condition and "could not evaluate" may not read as "complete".

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
cd "$REPO_ROOT" || exit 1

# ── leg 0: M6's commits have been read as a whole ──────────────────────────
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M6 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "leg 0: every M6 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "leg 0: scripts/check-milestone-review.sh could not be run (exit 127)"
else
  fail "leg 0: M6's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M6 to build the packet"
fi

# ── leg 0b: every row M6 leaves open is handed on ──────────────────────────
ho_rc=0
bash "$REPO_ROOT/scripts/check-milestone-handoff.sh" --milestone M6 || ho_rc=$?
if (( ho_rc == 0 )); then
  ok "leg 0b: every row M6 leaves open is handed on by a roadmap.md deferral row"
elif (( ho_rc == 3 )); then
  skip "leg 0b: M6's handoff (asked after M6's roadmap cell flipped to \`complete\`)"
elif (( ho_rc == 127 )); then
  fail "leg 0b: scripts/check-milestone-handoff.sh could not be run (exit 127)"
else
  fail "leg 0b: M6's handoff check refused (exit $ho_rc) -- its output above says why"
fi

have_cargo=1
command -v cargo >/dev/null 2>&1 || have_cargo=0

names_a_test() {
  local found
  found="$(git grep -ln -- "fn $1\b" 'crates/*/tests/*' 'crates/*/src/*' 2>/dev/null)" || return 1
  [[ -n "$found" ]] || return 1
  printf '%s' "$found"
}

run_counted() {
  local label="$1" min="$2"; shift 2
  local out passed
  if ! out="$(cargo test "$@" --quiet 2>&1)"; then
    fail "$label: the run itself failed"
    note "run: cargo test $*"
    printf '%s\n' "$out" | tail -20
    return 1
  fi
  passed="$(printf '%s\n' "$out" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s+0}')"
  if (( passed < min )); then
    fail "$label matched only $passed test(s), floor $min"
    note "a renamed, moved or #[ignore]d test leaves a filter green"
    return 1
  fi
  ok "$label ($passed test(s))"
}

# One requirement leg: the label, the test that asserts it, and what is missing
# when it does not exist yet.
leg() {
  local label="$1" test="$2" missing="$3" where
  if ! where="$(names_a_test "$test")"; then
    fail "$label: nothing asserts it -- no test named $test"
    note "$missing"
    return
  fi
  note "$label: asserted by $where"
  if (( ! have_cargo )); then
    fail "$label: a test of this name exists and no toolchain here can run it"
    return
  fi
  run_counted "$label" 1 --workspace "$test" || note "$label is not met"
}

leg "FR-51/NFR-20 (a coordinator killed under load loses nothing)" \
  a_killed_coordinator_loses_no_acknowledged_record \
  "needs a durable metadata log (ADR-0046) and a restart that replays it"
leg "task 13 (a coordinator whose lease expired fences itself)" \
  a_paused_coordinator_fences_itself \
  "needs the coordinator lease and its self-fencing poll"
# ⚠️ **And under M10's own pause** (`M6.14`, `M6.0`'s review): the leg above
# steps a fake clock, which a kill-only suite could satisfy; this one holds a
# renewal's response in the S3 model past the TTL, the fault `M10.8` built,
# so the lease is lost by a pause and not by the test's arithmetic.
leg "task 13a (a lease paused past its TTL by M10's injector fences its holder)" \
  a_lease_paused_past_its_ttl_fences_its_holder \
  "needs M10's pause fault with the coordinator lease as its subject"
leg "NFR-44 (a node with an empty disk serves correctly)" \
  an_empty_disk_node_serves_correctly \
  "needs cold start from the object-storage log and its newest snapshot"
leg "task 14 (a node boots degraded without its coordinator)" \
  a_node_boots_degraded_without_its_coordinator \
  "needs a boot path with no hard dependency on the log being reachable"
leg "NFR-22 (failover and cold rebuild timed separately)" \
  failover_and_cold_rebuild_are_measured_separately \
  "needs a hot standby to promote and a cold rebuild to time, each recorded"
# ⚠️ **The two numbers are recorded, not just measured** (`M6.0`'s review):
# a test that times both and writes neither would pass this leg's test while
# `M6.md`'s "records hot-standby and cold-rebuild numbers separately" went
# unmet.
for key in hot_standby_us cold_rebuild_us; do
  if grep -qE "^${key}: [0-9]+$" baselines/rto.txt 2>/dev/null; then
    ok "NFR-22: baselines/rto.txt records ${key}"
  else
    fail "NFR-22: baselines/rto.txt does not record ${key}"
    note "run the measurement with --nocapture and record its RTO line"
  fi
done
leg "rebuild equivalence (a rebuilt index answers as the original)" \
  a_rebuilt_index_answers_like_the_original \
  "needs a cold rebuild whose index is compared query by query, not byte by byte"
leg "FR-21 (committed offsets survive a restart)" \
  committed_offsets_survive_a_restart \
  "needs a durable GroupMetadataLog (M6.md task 7c)"

finish
