#!/usr/bin/env bash
# The M2 completion condition, exactly as `M2.md` states it: real clients
# complete produce and fetch round trips against a stub partition; every
# advertised API returns something other than UNSUPPORTED_VERSION (FR-2);
# the golden-byte corpus re-encodes byte-exactly; and the CRC differential
# against the scalar reference passes.
#
# ## Where this runs, and why not in pre-commit
#
# Standalone, at the milestone boundary — same as the three gates before
# it. NFR-56 gives the whole pre-commit suite 10 s; this builds the binary,
# runs the workspace suite under --all-features, and drives two real Kafka
# clients over TCP.
#
# ## A missing client is a skip, never a pass
#
# m1-complete.sh's Docker discipline, inherited verbatim: a developer
# without librdkafka or a JDK gets an honest "this leg did not run", not a
# green tick for a client that never connected. The skip lines come from
# scripts/kafka-client-harness.sh, which owns the client legs.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

MATRIX="docs/protocol-support.md"

# ── 0a. The public matrix exists ────────────────────────────────────────────
# First and tool-free, m1-complete.sh's ordering: the recorded artifact this
# gate vouches for must exist before anything needing cargo can be reached.
if [[ ! -f "$MATRIX" ]]; then
  fail "$MATRIX not found -- the public protocol-support matrix is an M2 deliverable"
  finish
fi
ok "the protocol-support matrix exists ($MATRIX)"

# ── 0b. Every M2 commit read as a whole ─────────────────────────────────────
# Above every skip and every tool requirement (M1.43's finding): this check
# needs neither cargo nor a client, so nothing below may gate it. The exit
# code is reported, not collapsed (m1-complete.sh's second finding).
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M2 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "every M2 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "scripts/check-milestone-review.sh could not be run (exit 127)"
  note "the gate is absent, not the review -- M2's coverage is unknown, not failing"
else
  fail "M2's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M2 to build the packet"
fi

if ! has_rust; then
  skip "everything below (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish

# run_counted <label> <min-tests> <cargo test args...>
#
# ⚠️ A cargo test filter that matches nothing exits 0 ("0 passed; N
# filtered out") -- so a leg that named its tests by filter alone would go
# green the day a refactor renamed them, which is the exact failure these
# legs exist to catch. The pass count is parsed and held to a floor.
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

# ── 1. The workspace suite, every feature on ────────────────────────────────
# --all-features is load-bearing: the snappy/gzip compression paths are
# feature-gated and M2.20's commit promised this gate would turn them on.
if cargo test --workspace --all-features --quiet >/dev/null 2>&1; then
  ok "cargo test --workspace --all-features"
else
  fail "cargo test --workspace --all-features failed"
  note "run it directly for the failure detail"
  finish
fi

# ── 2-4. The named legs of the completion condition ─────────────────────────
# Named individually even though step 1 ran them: these lines ARE the
# corpus, FR-2 and CRC legs, and each holds its pass count to a floor so a
# quietly renamed test cannot leave a filter green (run_counted above).
run_counted "golden-byte corpus (real librdkafka frames) re-encodes byte-exactly" 5 \
  -p oqueue-broker --test it corpus:: || finish
run_counted "every advertised (api, version) pair answers -- never 35, never a close" 1 \
  -p oqueue-broker --test it matrix:: || finish
run_counted "CRC-32C differential against the scalar reference" 1 \
  -p oqueue-checksum differential_against_the_scalar_reference || finish

# ── 5. Real clients round-trip against the served broker ────────────────────
# Delegated: the harness owns broker startup, client discovery, jar pinning
# and per-client skip lines (build.md rule 22 -- one implementation). It
# records each client that VERIFIED in target/harness/clients.txt; exit 0
# with an empty roster means every client was skipped, and a skip is never
# a round trip.
if ! bash "$REPO_ROOT/scripts/kafka-client-harness.sh"; then
  fail "a real Kafka client failed its round trip"
  finish
fi

CLIENT_ROSTER="target/harness/clients.txt"
have_librdkafka=0
have_java=0
if [[ -f "$CLIENT_ROSTER" ]]; then
  grep -q '^librdkafka ' "$CLIENT_ROSTER" && have_librdkafka=1
  grep -q '^java ' "$CLIENT_ROSTER" && have_java=1
fi

# ⚠️ The completion condition names BOTH clients. The final claim prints
# only when both round-tripped; anything less is said out loud instead --
# m1-complete.sh's discipline, where a skipped leg withholds the verdict
# rather than passing it.
if (( _FAILURES == 0 )) && (( have_librdkafka )) && (( have_java )); then
  ok "M2 completion condition holds"
elif (( _FAILURES == 0 )); then
  (( have_librdkafka )) || skip "librdkafka never ran -- the completion condition's client leg is unproven here"
  (( have_java )) || skip "the Java client never ran -- the completion condition's client leg is unproven here"
  warn "every check that could run passed, but the completion condition was NOT fully asserted"
fi
finish
