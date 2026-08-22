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
# ⚠️ Both clients set enable.idempotence=false, by decision rather than
# accident (M2.md's risk list): the idempotent path needs InitProducerId
# (key 22), which is M11's. docs/protocol-support.md records it.
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
  if python3 scripts/harness/rdkafka_roundtrip.py "$ADDR" > "$HARNESS_DIR/rdkafka.log" 2>&1 \
    && grep -q '^ROUND TRIP OK$' "$HARNESS_DIR/rdkafka.log"; then
    ok "librdkafka $LIBRDKAFKA_VERSION produce+fetch round trip"
    echo "librdkafka $LIBRDKAFKA_VERSION" >> "$ROSTER"
  else
    fail "librdkafka round trip failed"
    note "$(tail -5 "$HARNESS_DIR/rdkafka.log" 2>/dev/null || true)"
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
  if javac -cp "$HARNESS_DIR/$KAFKA_CLIENTS_JAR" -d "$HARNESS_DIR" scripts/harness/RoundTrip.java \
    && java -cp "$HARNESS_DIR:$HARNESS_DIR/$KAFKA_CLIENTS_JAR:$HARNESS_DIR/$SLF4J_JAR" \
        RoundTrip "$ADDR" > "$HARNESS_DIR/java.log" 2>"$HARNESS_DIR/java-stderr.log" \
    && grep -q '^ROUND TRIP OK$' "$HARNESS_DIR/java.log"; then
    ok "Java client (kafka-clients 3.9.1) produce+fetch round trip"
    echo "java kafka-clients-3.9.1" >> "$ROSTER"
  else
    fail "Java client round trip failed"
    note "$(tail -5 "$HARNESS_DIR/java.log" 2>/dev/null || true)"
    note "$(tail -3 "$HARNESS_DIR/java-stderr.log" 2>/dev/null || true)"
  fi
fi

finish
