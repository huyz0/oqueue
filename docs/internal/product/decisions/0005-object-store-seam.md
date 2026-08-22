# 0005. The `ObjectStore` seam

Status: accepted; 2026-08-23 (`M2.10`): two claims below are decision-time
facts the tree has overtaken — "the conditional write this trait does not
yet have" (it has: `Precondition` on `put` since `M1.5`/`M1.6`, so a
conformance assertion presents `IfMatches` rather than quiescing writes),
and the in-memory-backend split `M1.37` already annotated in the body. The
durability-flag obligation guarantee 1 leaves is `M3`'s, via `M1.58`
Date: 2026-08-16
Requirements: NFR-51

## Context

Object storage is this project's primary log storage, so the seam over it is the
one every other design decision leans on. It has to admit S3, GCS and an
in-memory implementation behind one trait, be chosen at startup by the composition
root (FR-50), and appear nowhere in `oqueue-core` as a concrete type
(NFR-51).

`contracts.md` rule 15 makes a new `pub trait` here a decision, and
`check-core-contract.sh` refuses the commit without this file.

⚠️ **FR-31 is deliberately not claimed.** It is the requirement that says S3,
GCS and in-memory sit behind one seam, and it is `M1`'s — ⚠️ the in-memory
one being `oqueue-core`'s `FakeObjectStore` rather than a second
`oqueue-store` backend, see *Alternatives* (`M1.37`) — `milestones/M0.md`
argues at length that M0 defines the seam without satisfying it, since a seam
with no backend behind it changes no outcome FR-31 describes. This ADR serves
NFR-51 only.

## Decision

Two methods, shaped as ADR-0002 decided — one `dyn`-compatible trait returning a
hand-written boxed future, both input lifetimes tied:

```rust
pub trait ObjectStore: Send + Sync + fmt::Debug {
    fn get<'a>(&'a self, key: &'a ObjectKey) -> BoxFuture<'a, Result<Vec<u8>>>;
    fn put<'a>(&'a self, key: &'a ObjectKey, payload: Vec<u8>) -> BoxFuture<'a, Result<()>>;
}
```

### What an implementor must guarantee

1. **When `put`'s future resolves `Ok`, the object is durable.** Not queued, not
   buffered — durable, in the sense that a process crash immediately afterwards
   does not lose it. This is what NFR-20 rests on. ⚠️ An in-memory backend
   cannot meet the crash clause and is not expected to: it is conformant for
   every guarantee a single process can express, and ⚠️ ~~`M1`'s suite skips
   this one explicitly rather than by omission~~ — **false, measured by
   `M1.37`'s review**: `Capabilities` has exactly two flags,
   `conditional_writes` and `ranged_reads`, neither about durability, no case
   in `cases()` exercises the crash clause, and `tests/it/fake.rs` asserts
   `report.skipped.is_empty()`. So it is skipped **by omission**, which is
   what this sentence promised it was not. The guarantee still holds as
   stated; what does not exist is the explicit skip, and whoever writes the
   durability case owes a capability flag to carry it.
2. ⚠️ **When `put`'s future resolves `Err`, the key's state is unknown.** Not
   "unchanged" — the object may have landed and the acknowledgement been lost.
   A caller must not treat a failed `put` as proof of absence, and this is
   precisely the case the commit protocol turns on.
3. **A `get` of a key that was `put` returns exactly those bytes**, or an error.
   Never a prefix, never a different version.
4. ⚠️ **An object is never partially visible.** A concurrent `get` sees the whole
   old object or the whole new one. Both S3 and GCS give this; an implementor
   that cannot must say so loudly, because the index assumes it.
5. **An empty object is a real object**, distinct from a missing one. A segment
   with no records is a legitimate thing to store, and conflating the two turns
   a normal state into a lookup failure.
6. ⚠️ **Strong read-after-write visibility, per key.** Once `put`'s future has
   resolved `Ok`, and **in the absence of a later `put` to that key**, every
   subsequent `get` — from any thread, any task, any node — returns that object
   rather than `ObjectNotFound` or an older version. Doc 04 §1 calls this one of
   the two guarantees that matter for log storage, and S3 and GCS both provide
   it.

   ⚠️ **Racing unconditioned overwrites are last-writer-wins, and this guarantee
   does not change that.** Two `put`s to one key, neither ordered before the
   other, both resolve `Ok` and one object survives — on S3, on GCS, and in
   `M1`'s in-memory implementation (`FakeObjectStore`; `M1.37`). A caller that needs to know it won must not
   overwrite; it must use the conditional write this trait does not yet have,
   which is the deferral recorded under *Alternatives*. A conformance assertion
   written from this guarantee must therefore quiesce writes to the key first,
   or it fails against every real backend.

   ⚠️ **Durability is not visibility, and guarantee 1 does not imply this one.**
   The gap is where a real defect lives: `M1` puts a chunk cache with
   single-flight *in front of* `get`, inside an implementor of this seam. A
   negative cache entry left by a speculative prefetch satisfies guarantees 1-5
   completely while returning `ObjectNotFound` for a segment the coordinator has
   already committed — and the fetch path cannot distinguish "not yet visible"
   from "lost", so it reports loss on an acknowledged record. That is NFR-20,
   the requirement everything else yields to.

   ⚠️ **The fake cannot catch this**, because it is strongly consistent by
   construction: every test above the seam passes either way. `M1`'s conformance
   suite is the only thing that can, and this guarantee is what it must assert.

### Payload type

`Vec<u8>`, deliberately, and this is the part most likely to change.

⚠️ **This narrows ADR-0002, which said the payload must be a type `oqueue-core`
owns.** `Vec<u8>` is `std`, owned by nobody. It satisfies that ADR's stated
*reason* — no dependency at the centre of the star — but not its stated
constraint, and the narrowing is deliberate rather than an oversight: a
core-owned newtype over `Vec<u8>` would buy a name and nothing else until
there is a reason to make it refcounted, and that reason lives in
`oqueue-buf`, downstream.

⚠️ The refcounted buffer that avoids copies belongs in `oqueue-buf`, which is
**downstream** of `oqueue-core` — so it cannot appear in this signature without
moving buffer pooling into the centre of the star, where every crate would
rebuild on every change to it. The copy `Vec<u8>` costs is not yet measured;
`M1` decides whether it matters with a backend in hand, and changing this then
is a contract change with its own commit, which is the correct cost.

## Alternatives considered

**A conditional-write method now** — `put_if_absent`, or a precondition
argument. Deferred, not rejected, and the deferral is uncomfortable: doc 10 #33
calls conditional-write semantics the highest-risk surface in the project, and
`M1`'s conformance suite exists largely to pin them. It is deferred because the
right shape depends on what S3's `If-None-Match` and GCS's generation
preconditions turn out to have in common, which is `M1`'s to discover.
⚠️ Adding it is a contract change under `contracts.md` rule 12 and will touch
this trait, its fake and every implementation in one commit.

**Ranged reads** — `get_range(key, offset..len)`. Deferred for the same reason
and with less risk: doc 12 makes ranged GETs central to the cost model, but the
shape is uncontroversial and nothing needs it until `M1` reads a segment.

**Streaming, rather than whole objects.** Rejected for now. A `Stream` in the
signature would make the trait harder to fake faithfully and harder to reason
about for the conditional-write cases, and segments are sized by this project
rather than by a client. If a segment ever exceeds what should be held in
memory, that is the signal to revisit.

**`bytes::Bytes` as the payload type.** Rejected: it puts an external dependency
in the one crate whose emptiness is load-bearing, and `check-layering.sh`
intersects only against *workspace* crate names, so nothing would have caught
it. See the Payload section.

**The fake in `oqueue-store`, beside the in-memory backend.** Rejected.
`contracts.md` rule 9 puts a fake beside its trait so a downstream crate can
test without depending on `oqueue-store`. **That half stands** — the fake is in
`oqueue-core` and every crate above the seam tests without an `oqueue-store`
dependency.

⚠️ **The other half was overtaken, and `M1.37` found it by looking for a
backend that does not exist.** ~~The two are different things that look alike:
`oqueue-store`'s in-memory backend is a *real implementation* that must pass
the same conformance suite as S3 (`testing.md` rule 6); this fake models no
failure and no latency at all.~~ Both clauses are now false. `M1.8` gave
`FakeObjectStore` a `FaultConfig` — latency, error storms,
`crash_after_put_before_ack` — and `M1.10` runs it through the whole
conformance suite at `Capabilities::FULL`, recorded `verified` in
`baselines/conformance-matrix.txt`. So M1 built **one** implementation that
does both jobs, and the separate in-memory backend was never written because
nothing was left for it to do.

⚠️ This is a dissolution, not a reversal: the reason for putting the fake in
`oqueue-core` is unchanged, and FR-31's "in-memory implementation for tests"
is satisfied — by a crate this ADR did not expect to satisfy it. What is gone
is the two-implementations premise, and with it the `testing.md` rule 6 that
pointed readers at the crate that never had one.

## Consequences

**Easy.** `bin/oqueue` holds an `Arc<dyn ObjectStore>` and picks a backend at
startup. Every crate above the seam is testable with no network, no container
and no credentials.

**Hard.** One heap allocation per call for the boxed future, and a copy of every
payload because it is a `Vec<u8>`. Neither is measured yet; both are the price
of a seam that works from the composition root.

⚠️ **Exactly one `ObjectStore` fake exists in this tree, and `M1` rewrites this
one rather than adding another beside it.** Two fakes with divergent conditional
-write semantics is the highest-risk defect class in the project — the failure
is silent, and it makes every test above the seam a test of the wrong thing.

⚠️ **What this fake does not buy.** ~~It models no latency, no failure, no
partial write and no conditional write. A test passing against it has shown
nothing about behaviour when a PUT fails *after* the object landed but before
the ack — which is the case the whole commit protocol turns on. That is `M1`'s
conformance suite, and until it exists, passing tests here are weaker evidence
than they look.~~

**`M1` built all of it, which is why this ADR's two-implementation premise
dissolved — `M1.37`.** `FaultConfig` (`M1.8`) models latency, error storms and
`crash_after_put_before_ack` — the exact PUT-lands-then-ack-is-lost case this
paragraph named as the one the commit protocol turns on and the fake could not
show. `put` takes a `Precondition` (`M1.5`, `M1.6`). The conformance suite
exists (`M1.10`) and this fake passes it at `Capabilities::FULL`, recorded
`verified` in `baselines/conformance-matrix.txt`.

⚠️ What it still does not buy is anything **cross-process**: guarantee 1's
crash clause is about a real process dying, and no single-process fake can
demonstrate that whatever faults it models. That is what real-backend
verification is for, and `M1.44` deferred it to `M15`.
