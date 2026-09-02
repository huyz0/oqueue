#!/usr/bin/env bash
# The M11 completion condition, as `M11.md` states it: a client conformance
# test with `enable.idempotence=true` produces successfully; a duplicate
# sequence number is never observable to a consumer (FR-14); a fenced
# producer epoch is refused; and M2's workaround no longer exists in the
# tree.
#
# ## Each leg is falsifiable or says it is not
#
# `M3.16`/`M10.15`'s precedent: a leg that cannot fail is worse than no leg,
# so each one below says what it actually asserts rather than what the
# milestone's prose suggests.
#
#   - **"Never observable to a consumer"** is asserted at the journal, not by
#     fetching: `sequencing::an_exact_replay_answers_success_with_the_recorded_offset`
#     shows a replay never reaches the journal at all -- `ADR-0031` point 2's
#     transparent-success mechanism, stronger than "a consumer would see one
#     copy" because there is structurally nothing past the allocator for a
#     consumer to ever read twice. This is the unconditional half; `M11.10`'s
#     harness leg (below) is the same property proven end to end, through a
#     real broker and a real consumer, when a real client is available to
#     drive it.
#   - **"Never observable"** is scoped to single-shard dedup, the way
#     `M11.10`'s own backlog row is: FR-15 (cross-partition atomicity,
#     transactions) is deferred, and a `transactional_id`-carrying call is
#     refused rather than answered as if understood. This gate does not
#     assert anything about a write spanning more than one partition.
#   - **The conformance leg is a skip, not a pass, when no client can run
#     it** -- `m1-complete.sh`'s discipline, inherited by `m2-complete.sh`
#     and again here: a developer without librdkafka gets an honest "this
#     leg did not run", not a green tick for a client that never connected.
#
# ## Where this runs, and why not in pre-commit
#
# Standalone, at the milestone boundary — the ten gates before it do the
# same. `NFR-56` gives the whole pre-commit suite 10 s; this runs the
# workspace suite and drives a real Kafka client over TCP besides.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

# ── 0a. Every M11 commit read as a whole ────────────────────────────────────
# Above every skip and every tool requirement (M1.43's finding, m3-complete.sh's
# precedent): this check needs no cargo, so nothing below may gate it.
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M11 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "every M11 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "scripts/check-milestone-review.sh could not be run (exit 127)"
  note "the gate is absent, not the review -- M11's coverage is unknown, not failing"
else
  fail "M11's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M11 to build the packet"
fi

# ── 0b. M2's workaround no longer exists in the tree ────────────────────────
# Above the cargo guard: this needs no toolchain, and M11.9/M11.11's own
# gate already runs it in pre-commit -- re-run here rather than trusted,
# the way M10.9's check-sans-io.sh leg is (a standalone gate should not
# assume the tree it is handed already passed every pre-commit check).
if bash "$REPO_ROOT/scripts/check-idempotence-enabled.sh" >/dev/null 2>&1; then
  ok "M2's enable.idempotence=false workaround no longer exists in the tree"
else
  fail "scripts/check-idempotence-enabled.sh failed -- the workaround, or something shaped like it, is back"
  note "run: scripts/check-idempotence-enabled.sh"
fi

if ! has_rust; then
  skip "everything below (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish

# run_counted <label> <min-tests> <cargo test args...>
#
# `m2-complete.sh`'s helper, `m3-complete.sh`/`m10-complete.sh`'s precedent:
# a cargo test filter that matches nothing exits 0 ("0 passed; N filtered
# out"), so a leg named by filter alone goes green the day a refactor
# renames the test. The pass count is parsed and held to a floor.
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

# ── 2. FR-14: a duplicate sequence is never observable to a consumer ───────
# ⚠️ Scoped, per this file's own header: single-shard dedup, and asserted at
# the journal rather than by fetching -- see above for why that is the
# stronger claim, not a weaker one.
run_counted "FR-14: a replay of an exact duplicate never reaches the journal at all" 1 \
  -p oqueue-coordinator --test it \
  sequencing::an_exact_replay_answers_success_with_the_recorded_offset || finish

# The allocator's own half of the same property, one layer down: a replay
# is answered from recorded state without ever being staged for an offset.
run_counted "FR-14: an exact replay answers the recorded offset without staging a new one" 1 \
  -p oqueue-coordinator --lib \
  allocator::admission::tests::an_exact_replay_answers_the_recorded_offset_without_a_new_one || finish

# ── 3. A fenced producer epoch is refused ───────────────────────────────────
# `M11.7`, `ADR-0031` point 5: a zombie incarnation of a producer, sending
# behind an epoch some later incarnation already moved past, is refused with
# real Kafka's own INVALID_PRODUCER_EPOCH rather than an ordinary gap.
run_counted "a fenced producer epoch is refused with the wire code Kafka uses for it" 1 \
  -p oqueue-broker --lib \
  produce::idempotent::a_zombie_epoch_is_refused_with_the_invalid_producer_epoch_wire_code || finish

# ── 4. A client conformance test with enable.idempotence=true, end to end ──
# Delegated: the harness owns broker startup, client discovery, and the
# per-client skip lines (build.md rule 22 -- one implementation). It records
# each client that VERIFIED in target/harness/clients.txt; a skip here is
# never mistaken for a round trip, the way m2-complete.sh's own leg is.
if ! bash "$REPO_ROOT/scripts/kafka-client-harness.sh"; then
  fail "a real Kafka client failed its round trip or its idempotent-producer conformance"
  finish
fi

CLIENT_ROSTER="target/harness/clients.txt"
have_librdkafka=0
have_idempotent_conformance=0
if [[ -f "$CLIENT_ROSTER" ]]; then
  grep -q '^librdkafka ' "$CLIENT_ROSTER" && have_librdkafka=1
  grep -q '^idempotent-conformance$' "$CLIENT_ROSTER" && have_idempotent_conformance=1
fi

# ⚠️ The completion condition names the conformance case specifically, not
# either client generally -- `have_librdkafka` alone would pass on the
# ordinary round trip while the idempotent-producer leg silently never ran.
if (( _FAILURES == 0 )) && (( have_librdkafka )) && (( have_idempotent_conformance )); then
  ok "M11 completion condition holds"
elif (( _FAILURES == 0 )); then
  (( have_librdkafka )) || skip "librdkafka never ran -- the conformance leg is unproven here"
  (( have_idempotent_conformance )) || skip "the idempotent-producer conformance leg never ran"
  warn "every check that could run passed, but the completion condition was NOT fully asserted"
fi
finish
