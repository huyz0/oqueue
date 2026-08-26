# `oqueue-broker` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, ⚠️ *not* `check-sans-io.sh` (this directory is exempt — the I/O shell lives here), `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-broker` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ **A composer.** `check-layering.sh` names this crate and `bin/oqueue` as the only two allowed to depend on workspace crates other than `oqueue-core`.
2. **Generic over its seams, not hardwired to them.** The concrete `ObjectStore` and `KeyProvider` are chosen by `bin/oqueue`; this crate names neither (NFR-51). ⚠️ **`Clock` is deliberately not claimed here** — not because the documents disagree, which `M1.29` found they do not, but because `check-sans-io.sh` says the real one lives here, so asserting the negative would be an invariant this crate is expected to break — the socket defect `M0.31` fixed. `check-sans-io.sh`'s exemption says where the real one *lives* (this crate); `architecture.md` says where concrete types are *chosen* (`bin/oqueue`); ADR-0004 takes no position on the real one's home. `README.md`'s Invariants row has the full reading.
3. ⚠️ **Backpressure between connections and the object-store write path is bespoke** — doc 05 §3 notes the real bottleneck is PUT throughput and request-rate limits, not socket I/O, and no crate provides that off the shelf.
4. ⚠️ **The handlers are async and the cluster is real** (`M3.14`). `Dispatcher::dispatch` awaits, produce PUTs and waits for a commit, fetch reads objects the index named. A handler added here that does I/O without awaiting the coordinator is one that answers before its record is durable, which is the ordering `ADR-0020` exists to hold.
5. ⚠️ **One region per `(topic, partition)` per object.** Regions carry no offsets — offsets are assigned at commit, after the object is written — so a second region for one partition leaves the read path nothing to say which an `ObjectRef` names. `produce.rs`'s `Pending` refuses the duplicate; `read.rs`'s `region_for` refuses the object. Both halves are needed: the first keeps this broker from writing one, the second handles bytes it did not write.

## Filling, `M2.17` onward

`M0.8` created the skeleton. `M2.17` added `connection`: framed reads,
pipelined handlers, in-order writes (the protocol's per-connection ordering
guarantee), bounded in-flight backpressure — generic over the stream, so
tests drive `tokio::io::duplex` and the first real socket appears in
`bin/oqueue`. `async-concurrency.md` binds everything here.

`M3.14` replaced `StubCluster` with [`Cluster`](src/cluster.rs): the topic
registry moved across unchanged — topic administration is nobody's milestone
yet — and the log half was deleted. [`flush`](src/flush.rs) seals one bundle,
PUTs it once and commits its spans; [`read`](src/read.rs) resolves
offset→object through the index and stamps the assigned base offset into the
batch on the way out, because the bytes were written before anyone knew what
order they landed in. [`ingest`](src/ingest.rs) is the verdict that keeps
unverified bytes out of object storage, and [`writer_id`](src/writer_id.rs) is
the per-process identity `ADR-0026`'s unconditional `put` rests on.
