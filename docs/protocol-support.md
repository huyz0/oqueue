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
| Produce | 0 | 3–13 | v0–2 removed by KIP-896. ⚠️ **A null topic name closes the connection** below v13, same as `Fetch` and `ListOffsets`. Exactly one RecordBatch (v2 magic) per partition; CRC-32C verified on ingest; base offset assigned by header rewrite. v13 addresses topics by id. |
| Fetch | 1 | 4–17 | ⚠️ **A null topic name closes the connection** below v13, for the reason the `ListOffsets` row gives — the field is nullable on the request wire and not on the response wire. Returns whole batches from the offset asked for, resolved through the index — ⚠️ **not from the log start**, which is what `M2`'s stub had to do and what this row said until `M3.14`. Long-polls on `max_wait_ms`/`min_bytes` (`M3.20`). Fetch sessions declined (`session_id` 0 → clients full-fetch). v13+ addresses topics by id. |
| `ListOffsets` | 2 | 1–9 | `EARLIEST` (-2) and `LATEST` (-1) answered from the coordinator's own index, never a cache — hazard H1, whose symptom is negative consumer lag. ⚠️ A **wall-clock** timestamp is refused with `UNSUPPORTED_VERSION` (35): there is no index by time, and the nearest offset would be a silent wrong answer. ⚠️ **Not code 43**, the obvious choice — the Java consumer maps that one to a null in `offsetsForTimes`, which an application cannot tell from a truthful "nothing at or after that time". ⚠️ **A null topic name closes the connection**: the field is nullable on the request wire and not on the response wire, so there is nothing parsable to answer with. v0 is a different message (an array of offsets per partition, pre-KIP-79) and is not advertised. |
| Metadata | 3 | 0–13 | `allow_auto_topic_creation` honoured from v4 (historical always-create below). Topic ids from v10. Single node, single partition per topic. |
| `SaslHandshake` | 17 | 0–1 | `M9.3`. Never flexible, at any version — a client cannot know what a flexible encoding means before this exchange tells it which mechanism it is using. `PLAIN` (`ADR-0032`) is the one enabled mechanism; anything else is refused with `UNSUPPORTED_SASL_MECHANISM` (33), the enabled-mechanism list sent regardless so the client can retry. |
| ApiVersions | 18 | 0–3 | Answered before anything else; unsupported versions get the v0-bodied `UNSUPPORTED_VERSION` fallback with the table populated. |
| `InitProducerId` | 22 | 0–4 | `M11.4`. A non-transactional call mints a fresh producer id at epoch zero, no coordinator round trip. A `transactional_id`-carrying call is refused with `INVALID_REQUEST` (42) — FR-15 (transactions) is deferred, not silently answered as if understood. |
| `SaslAuthenticate` | 36 | 0–2 | `M9.3`. ⚠️ **Refuses every exchange with `SASL_AUTHENTICATION_FAILED` (58)** — no credential source is configured yet; `M9.4` is what wires one. Not a stub: a broker with nothing to check a credential against must fail closed, and this is that failure, tested as real behaviour. |

Anything not in the table closes the connection: an unknown api key or an
unadvertised version has no response the client would parse, and outside
`ApiVersions`' fallback the only safe answer is a close.

## Known limitations, stated rather than discovered

- ⚠️ ~~**No idempotent produce**~~ — **no longer** (`M11.4`-`M11.9`).
  `InitProducerId` (key 22) mints an identity; a retried batch is
  deduplicated per `(producer, topic, partition)` against the recorded
  sequence, with `OUT_OF_ORDER_SEQUENCE_NUMBER`/`DUPLICATE_SEQUENCE_NUMBER`/
  `INVALID_PRODUCER_EPOCH` on the wire where real Kafka answers them.
  `enable.idempotence=true` is what the harness clients now run with — the
  Java client's own default since KIP-679; librdkafka's default is `false`
  (`M11.11`), so its harness sets the option explicitly rather than relying
  on one. ⚠️ **Single-shard dedup only** — FR-15
  (transactions, cross-partition atomicity) is still deferred, and a
  `transactional_id`-carrying call is refused rather than answered as if
  understood.
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
- ⚠️ ~~**`max_bytes` is not honoured.**~~ — **honoured since `M3.22`.** Both
  the request-level and per-partition fields are decoded, and the request's is
  a single allowance spent across every partition it names: a per-partition
  ceiling multiplied by a client-chosen count is not a ceiling. ⚠️ **Clamped at
  1 MiB** whatever a client asks for — `max_bytes` is an `i32`, and a broker
  that obliged a two-gigabyte request would let one frame decide how much
  memory it uses. ⚠️ **The first batch of a partition is served regardless**,
  as Kafka's own broker does: a partition whose first batch exceeds the
  allowance must stay readable, or a consumer parks at that offset forever —
  ⚠️ **once per *response***, not once per partition, or a request naming one
  partition two hundred times would collect two hundred whole batches.
  ⚠️ **The budget counts bytes *fetched*, not bytes returned**: a history batch
  lives inside a bundle covering every partition one flush wrote, so charging
  only the slice would let a read pull sixty-four whole bundles off the store
  to answer with a megabyte. ⚠️ **A partition whose read fails is charged what
  it *fetched*** — usually nothing, because a read the store refused pulled
  nothing; but a read that pulled a whole bundle and then could not use it
  spent exactly what a successful one would. ⚠️ **And it does not take the
  response's once-per-response exemption with it**: the exemption is one
  over-the-line read per **response**, and a failed read no longer consumes
  it, so it passes to the first partition that *can* use it — a guarantee
  about that one mechanism, not about the response as a whole. Once the
  failure cap below is spent, a partition whose object this request has not
  already touched is refused before it ever reaches the exemption, so a
  response can still end with no partition making progress at all.
  ⚠️ **Partitions after that one can still come back empty** when
  the budget is gone — the ordinary bound, which every client handles by
  polling again, and which both the Java consumer and librdkafka plan for by
  rotating the order they list partitions in. ⚠️ **What stops a failing partition being repeated for free is not
  the budget**: an object is fetched at most once per request *per byte range* — a tail entry
  names one partition's slice of a shared bundle, so a frame naming twenty
  partitions of one flush still issues twenty ranged GETs of that object, and
  what the cache stops is the *same* read being repeated — and after **two**
  failed object reads, any partition naming an object this request has not
  already touched is refused *without asking the store at all*. ⚠️ **A
  partition whose object is already in hand — a success, or one of the two
  failures itself — is still answered from that cache**, cap or no cap; the
  refusal is for objects the cache has never seen. ⚠️ **That last part is
  client-visible and worth planning
  for**: a consumer polling ten partitions of which two hold reaped objects
  can be answered `OFFSET_NOT_AVAILABLE` for the other eight. ⚠️ **Which
  partitions are refused depends on the order the frame lists them in** —
  the two failures have to come first for the refusal to reach anything, and
  clients rotate that order between polls — so this is a shape to be resilient
  to rather than a set to predict. The alternative was a frame's read rate against a
  struggling store rising with the fan-out of the client's own subscription.
  ⚠️ **And `fetch.min.bytes` is clamped to what the response can hold**,
  so a client naming a minimum above its own `max_bytes` does not park to its
  deadline on every poll of a full log.
- **A 404 from object storage is never "end of log".** Object ids are never
  reused, so an object the index named and the store does not have was
  **reaped**: the index is behind a deletion and those offsets are gone. The
  read answers `OFFSET_OUT_OF_RANGE` — ⚠️ **on the first miss, with no refresh
  and no retry**, because in a single-node broker the index a fetch reads *is*
  the coordinator's own and nothing removes an entry from it, so a second read
  would consult provably identical state and answer identically — not because
  it would cost a second GET (the object cache remembers a failure for the
  life of the request, so a retry against the same object is free), but
  because there is nothing a round trip to *this* index could learn that the
  first read did not already know. Doc 12 §4.6 asks for a coordinator round trip before this answer and
  `M7` is where it becomes real work, on a follower whose index is a cache and
  whose 404 is genuinely ambiguous. Never an empty partition, which is what a consumer
  reads as "I am caught up" while records it had not read are being deleted
  underneath it. ⚠️ **It is counted**: a nonzero rate means `M5`'s deletion
  delay is too short.
- **Read-your-writes is per connection.** A produce's ack carries a commit
  watermark, the connection remembers it, and the next fetch on that connection
  waits for the index to fold that far before it answers (hazard H2,
  `ADR-0023`). ⚠️ **The wait is bounded by the 5 s staleness budget, not by the
  client's `fetch.max.wait.ms`** — a non-blocking poll must not be able to
  switch a correctness guarantee off — and ⚠️ **a wait that runs out is an
  error, not an empty partition**: `OFFSET_NOT_AVAILABLE` (78), which the Java
  consumer's fetch error handling enumerates and retries. ⚠️ **Not
  `LEADER_NOT_AVAILABLE`**, the obvious choice — that code is *not* in that
  set, and falls through to an `IllegalStateException` out of `poll()`, so a
  refusal meant to protect a client would kill it. An empty partition is what a
  client reads as "nothing was written", which is the failure this exists to
  prevent.
  ⚠️ **Two connections are two sessions, and that is weaker than Kafka.** Real
  Kafka gives read-your-writes *across* connections: an acked produce is in the
  leader's log, so any consumer of that leader sees it once the high watermark
  advances, and no client has ever had to share a connection to get it. Here
  the guarantee is per connection because the session is where the watermark
  lives. ⚠️ **A genuine gap, not a non-gap** — invisible today only because a
  single-node broker's index *is* the coordinator's and therefore never behind,
  and real the moment a reader is a different process (`M7`).
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
