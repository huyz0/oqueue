# 0026. M3's real caller does not need a streaming writer, so it is not built here

Status: accepted; 2026-08-26: "Forecloses: nothing" overtaken by `M3.13` —
decision point 1's proviso does foreclose `KeyLayout::object_key` naming any
object this system writes, see `M3.md` task 19 and `key_layout.rs`
Date: 2026-08-26
Requirements: FR-31, FR-32

## Context

`ADR-0013` (`M1.16`) found two separate things while implementing S3 multipart:

1. `object_store`'s **public** API cannot condition a
   `CompleteMultipartUpload`. `CompleteMultipartMode` is `pub(crate)` and both
   public entry points hardcode `Overwrite`; upstream
   [`apache/arrow-rs-object-store#289`](https://github.com/apache/arrow-rs-object-store/issues/289)
   is the open issue.
2. `ObjectStore::put(key, payload: Vec<u8>, precondition)` cannot express a
   **streaming** writer at all — the whole payload is in memory by the time it
   is called, which is the opposite of doc 04 §5's motivating "unknown final
   size."

It deferred the streaming-writer-with-conditional-seal capability to **M3**, on
the explicit ground that `M3` would be *"where a new seam capability for it gets
designed, at the point something actually needs to call it, against whatever
`object_store`'s public API looks like by then"*, and that `M3` should
*"re-check this before choosing one of the other two paths"* — the two being a
hand-rolled SigV4 `CompleteMultipartUpload` and an `HttpConnector` interception.

`M3.13` is that caller. This ADR looks at it, and at the upstream state, before
choosing.

### The upstream state, re-checked

Against the pinned version this workspace builds, `object_store` **0.14.1**
(`Cargo.lock`), read from the vendored source rather than from the issue
tracker:

- `aws/client.rs`: `pub(crate) enum CompleteMultipartMode` — still crate-private.
- `aws/mod.rs`'s `put_multipart_opts` takes `PutMultipartOptions` and passes it
  to `create_multipart`; the two `complete_multipart` call sites in the public
  paths still pass `CompleteMultipartMode::Overwrite` literally, and the only
  `Create` is inside the copy-if-not-exists arm.
- `PutMultipartOptions` carries `tags`, `attributes` and `extensions`. No
  `PutMode`, and the doc on `extensions` says backends offered through the crate
  ignore them entirely.

⚠️ **So nothing upstream has changed, and this is checked rather than assumed** —
`ADR-0013`'s "wait for upstream" alternative is no cheaper today than it was.

### What `M3.13` actually needs

FR-32's flush accumulates produce batches in memory across N topics and writes
them as one object. Two properties decide this ADR:

- **The final size is known when the write starts.** The flush trigger — size
  or time — is what ends the accumulation, so the payload is complete before
  the first byte goes out. That is precisely the shape
  `ObjectStore::put(key, Vec<u8>, _)` already expresses, and precisely *not*
  the "unknown final size" doc 04 §5 motivates multipart with.
- **No precondition is wanted — *given* that the key is unique**, and that
  proviso is load-bearing rather than decorative. `ADR-0020` point 4 reserves
  object-storage conditional writes for two low-frequency control-plane uses and
  **never the offset stream**, and `M3.10`'s hazard H4 rests on object ids never
  being reused. If a flush writes a key nothing else can be writing, there is no
  race for a conditional seal to win.

  ⚠️ **The tree does not supply that key today, and this ADR does not pretend
  otherwise.** `KeyLayout::object_key` — the only key derivation that exists —
  aligns the offset *down* to a quantum by design, so every offset in one
  quantum computes the same key, and a property test pins exactly that. It also
  takes a `(topic, partition)`, which a bundled object spanning N topics is not.
  So `M3.13` cannot name its object with it, and the non-reuse `M3.10`'s H4
  already assumes is an obligation somebody has to meet rather than one the
  repository provides.

  ⚠️ **That obligation is named on `M3.13` by this ADR**, with doc 12 §4.6's own
  prescription — a UUID or a monotonic sequence — as the shape, and a test that
  two flushes never compute one key. Without it the decision below is wrong:
  an unconditional `put` onto a colliding key silently overwrites an object
  whose offsets are already committed and acknowledged, and a fetch of acked
  records returns somebody else's. A `Precondition` would at least have failed
  loudly. **Collision-free naming is what buys the right to omit it.**

⚠️ **The one part of the deferral that does have a caller is already built.** A
flush larger than `MultipartLimits::max_part_size` is handled by `M1.16`'s
unconditional multipart path, which splits and uploads without needing any of
this. ⚠️ **`max_part_size`, not `max_single_put`** — the two are far apart (8
MiB against 5 TiB on GCS), and `max_single_put` bounds *conditional* writes
only, so reading it as the multipart threshold would have `M3.13` size a flush
buffer six orders of magnitude above where multipart actually begins. What has no caller is the *conditional seal* and
the *unknown final size*, together.

## Decision

**The streaming-writer-with-conditional-seal capability is not built in M3, and
the deferral is re-targeted to `M5`.**

1. `M3.13` writes its bundled object with `ObjectStore::put` and no
   precondition. Nothing new crosses the seam.
   ⚠️ **Conditional on `M3.13` naming that object collision-free**, which
   `KeyLayout::object_key` does not do — it aligns an offset down to a quantum
   and takes a single `(topic, partition)`. Doc 12 §4.6 prescribes a UUID or a
   monotonic sequence; `M3.13`'s row carries the obligation and owes the test
   that two flushes never compute one key. If that turns out not to be
   achievable, this decision reopens, because the precondition is what would
   otherwise catch the collision.
2. Neither of `ADR-0013`'s two live mechanisms — hand-rolled SigV4 signing, or
   `HttpConnector` interception — is chosen, because choosing between them
   without a caller is what `ADR-0013` deferred in order to avoid. Both remain
   on the table, un-narrowed, for whoever needs them.
3. `M5` receives it. Compaction merges many input objects into one output whose
   final size is not known until the merge finishes — doc 04 §5's scenario
   exactly — and `M5.md` task 6 already says *"multipart output writer at the
   part size the cost model is built on"*. `roadmap.md`'s deferral table carries
   it there, and the upstream re-check goes with it.

⚠️ **This is a deferral that has now been justified twice by two different
arguments**, and they are not the same argument. `M1` deferred because it had no
caller. `M3` defers because the caller it was promised turned out not to need
it. That distinction is the reason this is an ADR and not a note: a future
reader finding "deferred again" is owed the second reason, or the deferral looks
like a habit.

## Alternatives considered

**Build the capability anyway, since M3 was told to.** Rejected on
`contracts.md` rule 3: a trait capability exists because more than one
implementation is expected *and* something calls it. This would have neither a
caller nor a test that is not a mock of one, and `ADR-0013`'s own reasoning —
design it "at the point something actually needs to call it" — argues against
building it at the point something turned out not to.

**Choose one of the two mechanisms now, so `M5` inherits a decision rather than
a choice.** Rejected: both were rejected in `M1` for reasons that have not
moved, and the tie-breaker between them is what the caller needs — whether the
seal must be `If-None-Match` or `If-Match`, whether one object or many, what
the retry story is on a lost seal. Deciding that against an imagined caller is
how a project acquires an architecture nobody chose, which is the failure
`ADR-0020`'s own Context names.

**Give `M3.13` a precondition anyway, for safety.** Rejected *given
collision-free naming*, and it is worth saying why rather than treating it as
obviously unnecessary: a conditional create would turn a retried flush after an
ambiguous timeout into a `PreconditionFailed` the caller must then interpret,
and `error-handling.md` rule 9 says a lost CAS is never retried automatically.
With a key nothing else writes, the retry is *already* safe — it either writes
the same bytes to the same unwritten key or overwrites its own partial write
with the complete one. A precondition would add a failure mode without removing
one. ⚠️ **Reverse the proviso and the rejection reverses with it**: if `M3.13`
cannot name collision-free, the precondition is the only thing standing between
a key collision and silently overwriting acknowledged records, and this
alternative becomes the decision.

## Consequences

**Makes easy:** `M3.13` is an ordinary `put`. FR-32's one-PUT claim needs no new
seam capability, and `M3` closes without a contract change it has no caller for.

**Makes hard:** `M5`'s compaction inherits the whole of `ADR-0013`'s problem —
the upstream gap, and the two unattractive mechanisms — at the point it needs a
conditional seal on a streaming output. That is later than `M1` hoped and the
right place, but it is not smaller.

**Forecloses:** nothing. Both mechanisms remain available, the upstream issue
remains the cheapest outcome if it closes, and no seam changed shape in the
meantime.
