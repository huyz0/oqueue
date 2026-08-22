# Kafka protocol support

What this broker answers, at which versions, and what it does not do yet.
The advertised table below is enforced twice: `oqueue-codec`'s version
tests pin it against the generated protocol spec, and
`crates/oqueue-broker/tests/it/matrix.rs` drives every advertised
`(api, version)` pair through the dispatcher and demands a real answer —
`scripts/gates/m2-complete.sh` runs both.

Verified against real clients by `scripts/kafka-client-harness.sh`:
librdkafka 2.15.0 and the Java client (kafka-clients 3.9.1) both complete
produce and fetch round trips.

| API | Key | Versions | Notes |
|---|---|---|---|
| Produce | 0 | 3–13 | v0–2 removed by KIP-896. Exactly one RecordBatch (v2 magic) per partition; CRC-32C verified on ingest; base offset assigned by header rewrite. v13 addresses topics by id. |
| Fetch | 1 | 4–17 | Returns whole batches from the log start for any in-range offset (consumers skip records below their fetch offset). Fetch sessions declined (`session_id` 0 → clients full-fetch). v13+ addresses topics by id. |
| Metadata | 3 | 0–13 | `allow_auto_topic_creation` honoured from v4 (historical always-create below). Topic ids from v10. Single node, single partition per topic. |
| ApiVersions | 18 | 0–3 | Answered before anything else; unsupported versions get the v0-bodied `UNSUPPORTED_VERSION` fallback with the table populated. |

Anything not in the table closes the connection: an unknown api key or an
unadvertised version has no response the client would parse, and outside
`ApiVersions`' fallback the only safe answer is a close.

## Known limitations, stated rather than discovered

- **No idempotent produce.** `InitProducerId` (key 22) is milestone M11's;
  test clients must set `enable.idempotence=false` (both harness clients
  do). This is the explicit decision `M2.md`'s risk list demanded.
- **No consumer groups.** `FindCoordinator`/`JoinGroup` et al. are not
  advertised; consumers must `assign()` rather than `subscribe()`.
- **Storage is a stub** — an in-memory single-node partition map. Durable
  object-storage-backed logs are M3's; offsets reset with the process.
- **Message format v0/v1 is refused, not converted** —
  `UNSUPPORTED_FOR_MESSAGE_FORMAT`, KIP-110's own precedent, and an error
  that names the format rather than steering clients at a compression
  setting.
- **No long-polling.** `max_wait_ms`/`min_bytes` are parsed and ignored;
  empty fetches return immediately.
- **Unknown topic ids answer `UNKNOWN_TOPIC_ID` (100)**; unknown names
  answer `UNKNOWN_TOPIC_OR_PARTITION` (3) — one refusal per addressing
  path, as real brokers do.

## The golden-byte corpus

`crates/oqueue-broker/tests/corpus/` holds request frames captured from
librdkafka 2.15.0 through `scripts/harness/capture-proxy.py`. Each frame
is decoded and re-encoded byte-exactly by
`crates/oqueue-broker/tests/it/corpus.rs`. These are real client bytes —
a fixture our own encoder generated would only prove the encoder agrees
with itself. Captures from other clients join the corpus as they are
taken; the file name records client and version.
