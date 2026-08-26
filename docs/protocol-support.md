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

⚠️ **The whole codec is `oqueue`'s own** (`ADR-0019`, `M2.28`-`M2.34`):
`kafka-protocol` answered every request through `M2.26`, then its
generated decoder's unbounded allocation from an untrusted array count
became a real single-packet DoS (a 64-byte Fetch frame demanding ~30 GB),
and `M2`'s milestone review ranked that blocking. Every request decoder
and response encoder below is hand-rolled in `oqueue-codec`, bounding
every count and length against the input before allocating anything;
`kafka-protocol` is now a `dev-dependency` differential oracle only, in
no runtime path anywhere in the workspace.

| API | Key | Versions | Notes |
|---|---|---|---|
| Produce | 0 | 3–13 | v0–2 removed by KIP-896. Exactly one RecordBatch (v2 magic) per partition; CRC-32C verified on ingest; base offset assigned by header rewrite. v13 addresses topics by id. |
| Fetch | 1 | 4–17 | Returns whole batches from the offset asked for, resolved through the index — ⚠️ **not from the log start**, which is what `M2`'s stub had to do and what this row said until `M3.14`. Long-polls on `max_wait_ms`/`min_bytes` (`M3.20`). Fetch sessions declined (`session_id` 0 → clients full-fetch). v13+ addresses topics by id. |
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
- ⚠️ ~~**Storage is a stub**~~ — **no longer** (`M3.14`). A produce seals one
  bundled object, writes it once, and is acknowledged only after its position
  is committed; a fetch resolves offset→object through the index. ⚠️ **The
  metadata log is still in memory**, so offsets do not survive a restart —
  `M6` owns the durable one (`roadmap.md`'s deferral table).
- **Compressed record batches are refused** — `UNSUPPORTED_COMPRESSION_TYPE`
  (76). ⚠️ Not a capability gap being papered over: a batch's `record_count` is
  what offsets are allocated from, and it cannot be checked against the records
  a batch actually holds when those records are behind a codec this broker does
  not implement. Believing it would let a client claim a thousand records in a
  batch holding one and leave a permanent hole in the log. `M8` decompresses.
  Both librdkafka and the Java client default to `compression.type=none`.
- **Message format v0/v1 is refused, not converted** —
  `UNSUPPORTED_FOR_MESSAGE_FORMAT`, KIP-110's own precedent, and an error
  that names the format rather than steering clients at a compression
  setting.
- ⚠️ ~~**No long-polling.**~~ — **`M3.20` wired it.** A fetch with nothing to
  return parks on the coordinator's index until a commit wakes it or the
  client's own `max_wait_ms` expires, and `min_bytes` is met by the whole
  response rather than per partition. ⚠️ **`fetch.min.bytes=0` answers
  immediately, empty included**, as a real broker does; a `min_bytes` larger
  than one response can hold is clamped to that, so it bounds the wait rather
  than guaranteeing it. ⚠️ **A wakeup for another partition costs no read**:
  the handler compares watermarks first and re-reads only when a partition
  this request asked about has actually moved. ⚠️ **And one request reads at
  most four times**: `min_bytes` is not always reachable — a page is capped at
  64 batches — so an unbounded park would let *producers* decide how much
  object storage one consumer's request costs. The cost of the cap is a
  response below `min_bytes` before the deadline, which every client already
  handles. ⚠️ **A `Fetch` naming no partitions is answered at once**, as a real
  broker does. ⚠️ **A refusal is never parked on**: an
  error a client can act on is not worth holding for half a second. ⚠️ **The
  park is capped at 60 s** whatever the client asks for — `max_wait_ms` is an
  `i32`, and a connection held for twenty-four days outlives the topic.
- **`max_bytes` is not honoured.** Both the request-level and per-partition
  fields are parsed and discarded; a fetch is bounded by a per-partition
  constant instead. `M3.22` owns the reader's byte budget.
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
