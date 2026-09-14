#!/usr/bin/env bash
# Real Kafka clients round-tripping against `oqueue serve` — the client half
# of `m2-complete.sh` (M2.25).
#
# Two clients, each optional in the lib.sh sense — a missing tool is a skip
# with a named remedy, never a silent pass, and never a failure:
#
#   - librdkafka, via python3 + confluent-kafka (the binding bundles the C
#     library, so importability IS librdkafka being present)
#   - the Java client, via a JDK + the pinned kafka-clients jar, fetched
#     into target/harness/ on first run and pinned by sha256 — same
#     discipline as m1-complete.sh's pinned MinIO image: an unpinned
#     dependency means two machines can disagree while both print pass
#
# ⚠️ Both clients ran with enable.idempotence=false until M11.9, by decision
# rather than accident (M2.md's risk list): the idempotent path needed
# InitProducerId (key 22), which M11.4-M11.8 built. Both now run with
# idempotence explicitly on (M11.11 -- the Java client gets it from its own
# KIP-679 default; librdkafka's own default is off, so its harness sets the
# option itself) — docs/protocol-support.md no longer records the
# limitation, and scripts/check-idempotence-enabled.sh is the gate that
# notices if the disable ever comes back.
#
# Exit: 0 when every client that could run passed; non-zero when any ran
# and failed. Prints one ok/skip line per client, and records each client
# that VERIFIED in target/harness/clients.txt -- the roster m2-complete.sh
# reads, so a skip here can never be mistaken for a round trip there.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

HARNESS_DIR="target/harness"
KAFKA_CLIENTS_JAR="kafka-clients-3.9.1.jar"
KAFKA_CLIENTS_URL="https://repo1.maven.org/maven2/org/apache/kafka/kafka-clients/3.9.1/$KAFKA_CLIENTS_JAR"
KAFKA_CLIENTS_SHA256="7568b998572d256f0b7bc0afdc1b7a2588b8b08415c62ce314c864a6851ae9d9"
SLF4J_JAR="slf4j-api-1.7.36.jar"
SLF4J_URL="https://repo1.maven.org/maven2/org/slf4j/slf4j-api/1.7.36/$SLF4J_JAR"
SLF4J_SHA256="d3ef575e3e4979678dc01bf1dcce51021493b4d11fb7f1be8ad982877c16a1c0"

BROKER_PID=""
cleanup() {
  if [[ -n "$BROKER_PID" ]]; then
    kill "$BROKER_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if ! has_rust; then
  skip "kafka client harness (no Rust toolchain to build oqueue with)"
  finish
fi

mkdir -p "$HARNESS_DIR"
ROSTER="$HARNESS_DIR/clients.txt"
: > "$ROSTER"
# ⚠️ **Truncated for the same reason the roster is.** `m4-complete.sh` reads
# this log to decide whether FR-21's survival was measured *this run*; a log
# left over from a previous one would let a host that cannot run the leg at
# all — no `confluent-kafka`, so the branch is skipped — report a stale
# `OFFSETS LOST` as if it had just measured it. That is the silent discharge
# the leg exists to prevent, arriving through the filesystem. Found by review.
: > "$HARNESS_DIR/offset-survival.log"

# ⚠️ **Comfortably above what a healthy run takes, and far below a hang.**
# `M4.38` measured the slowest stage of either group script at 6.5 s and the
# whole run well under a minute; both scripts' own settle windows are 30 s per
# stage across three stages. 300 s leaves a wide margin for a loaded host
# while still turning a stalled round into a red gate inside five minutes
# instead of never. ⚠️ A ceiling, so raising it weakens the guard
# (non-negotiable 2).
GROUP_CONFORMANCE_CEILING_S=300

# ⚠️ **Every leg that drives a real client is bounded, and `M4.44` bounded
# only the two group scripts.** Its commit body argued "both wrapped commands
# are leaf processes, which review confirmed by reading both scripts" — true
# of the two it wrapped, and the reason the rest went unexamined. ⚠️ **No
# count is written here on purpose**: `run_bounded`'s own header records an
# earlier sentence that "carried a count, which was wrong and would have
# drifted on the next script added", and the rule is the class, not the
# roster — **anything below that starts a real Kafka client gets a ceiling**.
#
# ⚠️ **The hang is not a property of the group scripts**: `M4.38` measured it
# as librdkafka's teardown at interpreter exit after an assertion escapes
# with a client still alive, and every Python leg here has that shape — each
# raises on its own deadline with a live `Consumer`/`Producer` in scope. The
# Java legs are bounded too; their client is not librdkafka, but a leg that
# can wait on a broker can stall on one.
#
# ⚠️ **Measured, so the margin is a fact rather than a guess.** One healthy
# run in the container: round trip 1 s, idempotent conformance 1 s, TLS +
# SASL/PLAIN 8 s (the group legs, on their own ceiling, 13 s). 120 s is
# fifteen times the slowest of those and turns a stalled leg into a red gate
# in two minutes instead of never. ⚠️ A ceiling, so raising it weakens the
# guard (non-negotiable 2).
CLIENT_LEG_CEILING_S=120

# ⚠️ **The offset-survival leg needs its own, and the healthy runtime is the
# wrong thing to size it against.** That leg runs 4 s healthy, but it stacks
# its own deadlines without any of them failing the run: 10 s to see a broker
# listen, 10 s to flush, a 30 s consume loop, 10 s for `committed`, 10 s to
# reap the broker, then a second broker (10 s), a second `committed` (10 s)
# and a second reap (10 s) — about 100 s of legitimate worst case. Under
# `CLIENT_LEG_CEILING_S` a loaded host running every stage near its own
# deadline would be SIGKILLed and reported as a hang, for a run that was
# about to answer `OFFSETS LOST` correctly. Found by review. 300 s is three
# times that budget, and matches the group ceiling above rather than
# inventing a third number. ⚠️ Also a ceiling (non-negotiable 2).
SURVIVAL_LEG_CEILING_S=300

# ── The broker under test ───────────────────────────────────────────────────
if ! cargo build -p oqueue --quiet; then
  fail "cargo build -p oqueue failed -- no broker to test against"
  finish
fi

BROKER_LOG="$HARNESS_DIR/broker.log"
./target/debug/oqueue serve 127.0.0.1:0 > "$BROKER_LOG" 2>&1 &
BROKER_PID=$!

ADDR=""
for _ in $(seq 100); do
  ADDR="$(awk '/^listening on /{print $3; exit}' "$BROKER_LOG" 2>/dev/null || true)"
  [[ -n "$ADDR" ]] && break
  if ! kill -0 "$BROKER_PID" 2>/dev/null; then
    fail "oqueue serve exited before listening"
    note "$(tail -3 "$BROKER_LOG" 2>/dev/null || true)"
    finish
  fi
  sleep 0.1
done
if [[ -z "$ADDR" ]]; then
  fail "oqueue serve never printed its listening address"
  finish
fi
note "broker under test at $ADDR (log: $BROKER_LOG)"

# ── librdkafka ──────────────────────────────────────────────────────────────
if ! command -v python3 >/dev/null 2>&1; then
  skip "librdkafka round trip (no python3; install python3 + confluent-kafka)"
elif ! python3 -c 'import confluent_kafka' 2>/dev/null; then
  skip "librdkafka round trip (confluent-kafka not importable; pip install confluent-kafka)"
else
  LIBRDKAFKA_VERSION="$(python3 -c 'import confluent_kafka; print(confluent_kafka.libversion()[0])')"
  roundtrip_rc=0
  run_bounded "$CLIENT_LEG_CEILING_S" \
    python3 scripts/harness/rdkafka_roundtrip.py "$ADDR" \
    > "$HARNESS_DIR/rdkafka.log" 2>&1 || roundtrip_rc=$?
  if (( roundtrip_rc == 0 )) \
    && grep -q '^ROUND TRIP OK$' "$HARNESS_DIR/rdkafka.log"; then
    ok "librdkafka $LIBRDKAFKA_VERSION produce+fetch round trip"
    echo "librdkafka $LIBRDKAFKA_VERSION" >> "$ROSTER"
  elif (( roundtrip_rc == 124 )); then
    # ⚠️ A ceiling is not a conformance failure, and reporting it as one sends
    # the reader after a protocol bug that is not there — the same correction
    # review made to the group leg below. A killed process usually leaves a
    # log it never finished writing, so the tail is often empty.
    fail "librdkafka round trip did not finish in ${CLIENT_LEG_CEILING_S}s"
    note "a client that hung rather than a round trip that answered wrongly"
    note "$(tail -5 "$HARNESS_DIR/rdkafka.log" 2>/dev/null || true)"
  else
    fail "librdkafka round trip failed"
    note "$(tail -5 "$HARNESS_DIR/rdkafka.log" 2>/dev/null || true)"
  fi

  # ── idempotent-producer conformance (M11.10) ──────────────────────────────
  # ⚠️ Same client, same importability check above; a separate `elif` chain
  # would re-test what this `else` branch already established. ⚠️ **Its own
  # broker**, not `$ADDR` — a real client's post-bootstrap connections use
  # `Metadata`'s advertised address, so the frame-dropping proxy this leg
  # needs has to be what the broker advertises from the start, which
  # `idempotent_conformance.py` arranges by starting both itself.
  #
  # ⚠️ **Two of `M11.10`'s three legs, by real-client conformance — the
  # third is provably unreachable that way and is named here rather than
  # silently skipped.** "Produces successfully" is the ordinary round trip
  # two lines up, running with `enable.idempotence` explicitly on (`M11.11`
  # -- librdkafka does not default to it); "a duplicate-sequence test
  # observes deduplication" is this leg. "A
  # fenced epoch is refused" is not reachable from any real,
  # standards-compliant *non-transactional* client: `M11.4`'s
  # `InitProducerId` handler always mints a fresh id at epoch zero for a
  # non-transactional call, and the only way a real client ever presents an
  # *existing* id at a *different* epoch is the transactional `InitProducerId`
  # flow (naming `transactional.id`), which that same handler refuses
  # outright (FR-15, deferred) — so no compliant client can ever be the
  # zombie this leg would need. `oqueue-broker`'s own
  # `produce/idempotent.rs::a_zombie_epoch_is_refused_with_the_invalid_producer_epoch_wire_code`
  # is where that property is verified, at the fidelity actually available:
  # a hand-rolled wire-level client, the same tool `M11.7`'s own admission
  # tests already use for exactly this reason.
  idempotent_rc=0
  run_bounded "$CLIENT_LEG_CEILING_S" \
    python3 scripts/harness/idempotent_conformance.py \
    > "$HARNESS_DIR/idempotent.log" 2>&1 || idempotent_rc=$?
  if (( idempotent_rc == 0 )) \
    && grep -q '^DEDUPLICATED OK$' "$HARNESS_DIR/idempotent.log"; then
    ok "librdkafka idempotent-producer conformance (a lost ack is deduplicated, not doubled)"
    echo "idempotent-conformance" >> "$ROSTER"
  elif (( idempotent_rc == 124 )); then
    # ⚠️ **This leg starts its own broker**, so a ceiling here leaves that
    # process behind — `run_bounded` signals the direct child only, and this
    # script's EXIT trap covers only the broker *it* started. ⚠️ Its
    # frame-dropping proxy is an `asyncio.start_server` inside the same
    # interpreter, so that one dies with the child; review corrected a first
    # version of this comment that named it too. `M4.61`.
    fail "idempotent-producer conformance did not finish in ${CLIENT_LEG_CEILING_S}s"
    note "a client that hung rather than a duplicate that was not deduplicated"
    note "$(tail -10 "$HARNESS_DIR/idempotent.log" 2>/dev/null || true)"
  else
    fail "idempotent-producer conformance failed"
    note "$(tail -10 "$HARNESS_DIR/idempotent.log" 2>/dev/null || true)"
  fi

  # ── TLS + SASL/PLAIN (M4.18, FR-40/FR-4/FR-45) ────────────────────────────
  # ⚠️ **The leg `M9.21` was waiting for.** `M9` built TLS termination, the
  # `SASL/PLAIN` mechanism, topic grants and quotas and wired none of them
  # into `bin/oqueue serve` — every `Dispatcher` in the tree was built with
  # plain `new`, so a broker from `M9`'s own crate answered every request
  # unauthenticated over cleartext. Only a real client connecting can show the
  # wiring is there: a unit test can assert a builder was called, not that a
  # password on the wire is checked.
  #
  # ⚠️ **Its own broker, because the certificate has to name the address the
  # client dials** — the shared `$ADDR` broker above runs cleartext on
  # purpose, since every other leg needs it that way.
  #
  # ⚠️ **`openssl` is the one extra tool**, and a missing one is a skip like
  # any other absent client rather than a failure.
  if ! command -v openssl >/dev/null 2>&1; then
    skip "TLS + SASL/PLAIN round trip (no openssl to make a test certificate)"
  else
    tls_rc=0
    run_bounded "$CLIENT_LEG_CEILING_S" \
      python3 scripts/harness/tls_sasl.py \
      > "$HARNESS_DIR/tls-sasl.log" 2>&1 || tls_rc=$?
    if (( tls_rc == 0 )) \
      && grep -q '^TLS SASL OK$' "$HARNESS_DIR/tls-sasl.log"; then
      ok "librdkafka over TLS with SASL/PLAIN (and refused without a credential)"
      echo "tls-sasl" >> "$ROSTER"
    elif (( tls_rc == 124 )); then
      # ⚠️ **Its own broker too**, for the certificate's sake — so the same
      # leak as the leg above when this ceiling fires.
      fail "TLS + SASL/PLAIN round trip did not finish in ${CLIENT_LEG_CEILING_S}s"
      note "a client that hung rather than a credential that was wrongly accepted"
      note "$(tail -10 "$HARNESS_DIR/tls-sasl.log" 2>/dev/null || true)"
    else
      fail "TLS + SASL/PLAIN round trip failed"
      note "$(tail -10 "$HARNESS_DIR/tls-sasl.log" 2>/dev/null || true)"
    fi
  fi

  # ── offset survival across a broker restart (M4.17, FR-21) ────────────────
  # ⚠️ **Reported, never asserted — and never as a pass.** FR-21's own
  # verification method is "offsets survive a full broker fleet restart" and
  # they do not: `serve` wires `FakeGroupMetadataLog`, an in-memory `Vec`, so
  # a process restart loses every committed offset. That is a recorded
  # deferral (`ADR-0035`; `roadmap.md`'s deferred-into-a-later-milestone
  # table receives the durable engine in `M6`), and `serve` warns about it on
  # startup. A green harness must not be readable as FR-21 being verified, so
  # today's outcome is a **skip** naming the reason.
  #
  # ⚠️ **It fails if the offsets start surviving**, which is the point: the
  # good news must not pass unnoticed either. `check-portability.sh`'s
  # both-directions discipline, applied to a requirement rather than to
  # prose — the day `M6` lands, this leg says so and asks to be promoted to
  # an assertion.
  survival_rc=0
  run_bounded "$SURVIVAL_LEG_CEILING_S" \
    python3 scripts/harness/offset_survival.py \
    > "$HARNESS_DIR/offset-survival.log" 2>&1 || survival_rc=$?
  if (( survival_rc == 124 )); then
    # ⚠️ **Before the outcome branches**, because a killed run writes neither
    # marker and would otherwise fall into "reported neither outcome" — which
    # reads as the leg answering ambiguously rather than as it never
    # finishing.
    fail "offset-survival leg did not finish in ${SURVIVAL_LEG_CEILING_S}s"
    note "it starts and restarts a broker of its own; a ceiling here leaves that behind"
    note "$(tail -10 "$HARNESS_DIR/offset-survival.log" 2>/dev/null || true)"
  elif (( survival_rc == 0 )); then
    if grep -q '^OFFSETS SURVIVED$' "$HARNESS_DIR/offset-survival.log"; then
      fail "committed offsets now survive a broker restart — FR-21 is satisfiable"
      note "promote this leg to an assertion and close FR-21's restart test"
      note "$(tail -3 "$HARNESS_DIR/offset-survival.log" 2>/dev/null || true)"
    elif grep -q '^OFFSETS LOST$' "$HARNESS_DIR/offset-survival.log"; then
      skip "offset survival across a broker restart (FR-21 needs M6's durable group metadata log, ADR-0035)"
      note "$(grep -E '^committed offset' "$HARNESS_DIR/offset-survival.log" | tail -2)"
    else
      fail "offset-survival leg reported neither outcome"
      note "$(tail -5 "$HARNESS_DIR/offset-survival.log" 2>/dev/null || true)"
    fi
  else
    fail "offset-survival leg failed before it could measure anything"
    note "$(tail -10 "$HARNESS_DIR/offset-survival.log" 2>/dev/null || true)"
  fi

  # ── consumer-group conformance (M4.17) ────────────────────────────────────
  # ⚠️ **Its own leg rather than folded into the round trip above**, because
  # it asserts something the round trip cannot: `rdkafka_roundtrip.py` uses
  # `assign()` and never issues a `JoinGroup`, so nothing here exercised the
  # group protocol with a real client until this. FR-20's invariant — a
  # partition is revoked by its previous owner before being handed on — is
  # only observable across a membership *change*, so this drives an initial
  # join, a consumer added, and a consumer removed.
  #
  # ⚠️ **A broker defect came out of writing it**, invisible to the unit
  # suite: the `SyncGroup` assignment barrier held no generation, so a
  # follower syncing for generation N that arrived before its leader was
  # answered from N-1's map — which, for a consumer that joined during N-1,
  # holds its own *empty* slice. A consumer added to a working group was
  # told it owned nothing and stayed idle. A second divergence was found
  # alongside it (the newest member became every group's leader) and fixed,
  # but this harness passes without that fix, so it is pinned by unit tests
  # rather than claimed here.
  # ⚠️ **Bounded, because its own deadline is not enough** — `M4.44`.
  # `settle` raises at `SETTLE_SECONDS`, and the process then hangs inside
  # librdkafka's teardown at interpreter exit, with or without a `finally`
  # that closes; `M4.38` measured that under two different timeout settings.
  # A broker regression that leaves a round open therefore stalls this gate
  # rather than failing it, and a job that never ends is worse than a red
  # one. `run_bounded` rather than `timeout(1)`, which stock macOS does not
  # ship — `lib.sh`'s own note, and `sha256_stdin`'s precedent.
  groups_rc=0
  run_bounded "$GROUP_CONFORMANCE_CEILING_S" \
    python3 scripts/harness/rdkafka_groups.py "$ADDR" \
    > "$HARNESS_DIR/rdkafka-groups.log" 2>&1 || groups_rc=$?
  # ⚠️ **A ceiling is not a conformance failure, and reporting it as one sends
  # the reader after a protocol bug that is not there** — the log a killed
  # process never finished writing is usually empty, so the `note` below was
  # blank. Found by review.
  if (( groups_rc == 0 )) \
    && grep -q '^GROUP CONFORMANCE OK$' "$HARNESS_DIR/rdkafka-groups.log"; then
    ok "librdkafka consumer-group conformance (join, add, remove; no partition assigned twice)"
    echo "librdkafka-groups" >> "$ROSTER"
  elif (( groups_rc == 124 )); then
    fail "librdkafka consumer-group conformance did not finish in ${GROUP_CONFORMANCE_CEILING_S}s"
    note "a round the broker never closed, or a client that hung past its own settle deadline"
    note "$(tail -10 "$HARNESS_DIR/rdkafka-groups.log" 2>/dev/null || true)"
  else
    fail "librdkafka consumer-group conformance failed"
    note "$(tail -10 "$HARNESS_DIR/rdkafka-groups.log" 2>/dev/null || true)"
  fi
fi

# ── the Java client ─────────────────────────────────────────────────────────
# fetch_jar <jar> <url> <sha256>
# rc 1: not fetchable (no network, no curl) -- the caller skips.
# rc 2: fetched bytes do not match the pin -- the caller FAILS: a hash
#       mismatch is a tamper-or-upstream-change signal, and a pin that can
#       only ever skip is not a pin. The bad file is deleted so a later run
#       re-fetches rather than re-reporting a stale partial download.
fetch_jar() {
  local jar="$1" url="$2" sha="$3" got
  if [[ ! -f "$HARNESS_DIR/$jar" ]]; then
    command -v curl >/dev/null 2>&1 || return 1
    curl -sfL -o "$HARNESS_DIR/$jar" "$url" || return 1
  fi
  # lib.sh's portable hash (sha256sum or shasum): macOS is first-class
  # (portability.md), and it ships only shasum.
  got="$(sha256_stdin < "$HARNESS_DIR/$jar")" || return 1
  if [[ "$got" != "$sha" ]]; then
    rm -f "$HARNESS_DIR/$jar"
    return 2
  fi
}

jar_rc=0
if ! command -v java >/dev/null 2>&1 || ! command -v javac >/dev/null 2>&1; then
  skip "Java client round trip (no JDK; install one and re-run)"
elif { fetch_jar "$KAFKA_CLIENTS_JAR" "$KAFKA_CLIENTS_URL" "$KAFKA_CLIENTS_SHA256" || jar_rc=$?; \
       (( jar_rc == 0 )) && { fetch_jar "$SLF4J_JAR" "$SLF4J_URL" "$SLF4J_SHA256" || jar_rc=$?; }; \
       (( jar_rc != 0 )); }; then
  if (( jar_rc == 2 )); then
    fail "a pinned jar's sha256 does not match -- tampering or an upstream change, never a skip"
  else
    skip "Java client round trip (pinned jars not fetchable from Maven Central)"
  fi
else
  # ⚠️ **Compiled first and run under a ceiling**, the shape the Java group
  # leg below already uses — `javac` inside the bounded command would make a
  # compile failure and a hang the same exit code, and `200` is what keeps
  # them apart.
  jroundtrip_rc=0
  if javac -cp "$HARNESS_DIR/$KAFKA_CLIENTS_JAR" -d "$HARNESS_DIR" \
      scripts/harness/RoundTrip.java 2>"$HARNESS_DIR/java-stderr.log"; then
    run_bounded "$CLIENT_LEG_CEILING_S" \
      java -cp "$HARNESS_DIR:$HARNESS_DIR/$KAFKA_CLIENTS_JAR:$HARNESS_DIR/$SLF4J_JAR" \
      RoundTrip "$ADDR" > "$HARNESS_DIR/java.log" \
      2>>"$HARNESS_DIR/java-stderr.log" || jroundtrip_rc=$?
  else
    jroundtrip_rc=200
  fi
  if (( jroundtrip_rc == 0 )) \
    && grep -q '^ROUND TRIP OK$' "$HARNESS_DIR/java.log"; then
    ok "Java client (kafka-clients 3.9.1) produce+fetch round trip"
    echo "java kafka-clients-3.9.1" >> "$ROSTER"
  elif (( jroundtrip_rc == 124 )); then
    fail "Java client round trip did not finish in ${CLIENT_LEG_CEILING_S}s"
    note "a client that hung rather than a round trip that answered wrongly"
    note "$(tail -5 "$HARNESS_DIR/java.log" 2>/dev/null || true)"
  elif (( jroundtrip_rc == 200 )); then
    fail "RoundTrip.java did not compile"
    note "$(tail -5 "$HARNESS_DIR/java-stderr.log" 2>/dev/null || true)"
  else
    fail "Java client round trip failed"
    note "$(tail -5 "$HARNESS_DIR/java.log" 2>/dev/null || true)"
    note "$(tail -3 "$HARNESS_DIR/java-stderr.log" 2>/dev/null || true)"
  fi

  # ── consumer-group conformance, the reference implementation (M4.17) ──────
  # ⚠️ **Both clients, because they are not interchangeable.** Their assignors
  # are separate implementations of the same protocol, and a broker can
  # satisfy one while starving the other — so a group leg that ran only
  # librdkafka would be evidence about librdkafka, not about the protocol.
  jgroups_rc=0
  if javac -cp "$HARNESS_DIR/$KAFKA_CLIENTS_JAR" -d "$HARNESS_DIR" \
      scripts/harness/GroupRebalance.java 2>"$HARNESS_DIR/java-groups-stderr.log"; then
    run_bounded "$GROUP_CONFORMANCE_CEILING_S" \
      java -cp "$HARNESS_DIR:$HARNESS_DIR/$KAFKA_CLIENTS_JAR:$HARNESS_DIR/$SLF4J_JAR" \
      GroupRebalance "$ADDR" > "$HARNESS_DIR/java-groups.log" \
      2>>"$HARNESS_DIR/java-groups-stderr.log" || jgroups_rc=$?
  else
    # ⚠️ A distinct code, so the compile failure is not reported as a run
    # that answered wrongly -- the stderr log holds only `javac`'s output.
    jgroups_rc=200
  fi
  if (( jgroups_rc == 0 )) \
    && grep -q '^GROUP CONFORMANCE OK$' "$HARNESS_DIR/java-groups.log"; then
    ok "Java client consumer-group conformance (join, add, remove; no partition assigned twice)"
    echo "java-groups" >> "$ROSTER"
  elif (( jgroups_rc == 124 )); then
    fail "Java client consumer-group conformance did not finish in ${GROUP_CONFORMANCE_CEILING_S}s"
    note "a round the broker never closed, or a client that hung past its own settle deadline"
    note "$(tail -10 "$HARNESS_DIR/java-groups.log" 2>/dev/null || true)"
  elif (( jgroups_rc == 200 )); then
    fail "GroupRebalance.java did not compile"
    note "$(tail -5 "$HARNESS_DIR/java-groups-stderr.log" 2>/dev/null || true)"
  else
    fail "Java client consumer-group conformance failed"
    note "$(tail -10 "$HARNESS_DIR/java-groups.log" 2>/dev/null || true)"
    note "$(tail -5 "$HARNESS_DIR/java-groups-stderr.log" 2>/dev/null || true)"
  fi
fi

finish
