# 0008. `object_store` as the client library `oqueue-store`'s backends are built on

Status: accepted; 2026-08-23 (`M2.10`, closing `M1.56`): the Consequences'
"`M1.12`'s retry policy configures `RetryConfig` rather than writing a
backoff loop" did not happen — what shipped is a sans-I/O `RetryPolicy` in
`oqueue-core` that **no caller invokes yet**, above `object_store`'s own
unconfigured vendor retry (measured live: `max_retries: 10`,
`retry_timeout: 180s`). No double-retry exists today for exactly that
reason. Wiring `RetryConfig` from the policy's constants belongs to the
seam's first real attempt-loop caller — `M3`'s composition — not to a
milestone with nothing to drive it
Date: 2026-08-16
Requirements: FR-31

## Context

`M1` gives `oqueue-store` real S3 and GCS backends behind the `ObjectStore`
seam ADR-0005 shaped and ADR-0002 chose the async signature for. Decision #3
(`roadmap.md`) blocks that code: which crate does the actual HTTP/signing/retry
work underneath it — the Apache Arrow `object_store` crate, Apache OpenDAL, or
the provider SDKs (`aws-sdk-s3`, `google-cloud-storage`) directly.

Three things this milestone's own risk section (`milestones/M1.md`) makes the
choice turn on, in order of weight:

1. **Native conditional-write support.** Doc 10 #33 calls conditional-write
   semantics the highest-risk surface in the project — the whole commit
   protocol rests on S3's `If-Match`/`If-None-Match` and GCS's
   `ifGenerationMatch` behaving as documented, and `M1`'s conformance suite
   exists to pin that behaviour. A client library that does not expose these
   as a first-class primitive pushes that risk into hand-rolled per-backend
   header code this project would then have to get right itself, with a
   correctness-critical property depending on it.
2. **One trait across S3 and GCS**, so `oqueue-store`'s backends are two
   implementations of one shape rather than two unrelated clients wrapped
   after the fact.
3. **Retry/backoff already centralized**, since `M1.12` (the retry policy
   driven by error class) needs a place to hook classification in, not a
   second retry loop competing with one buried in the client.

[Doc 05](../../../researches/05-rust-ecosystem.md) §1 surveyed all three in
depth on 2026-08-12; the numbers and API-shape claims below are sourced from
there rather than re-derived here.

## Decision

**`object_store` (the Apache Arrow project's crate)**, pinned in
`[workspace.dependencies]`, for both the S3 and GCS backends.

The primitive that decides it: `object_store`'s `PutMode` enum gives
`Create` (atomic put-if-absent) and `Update(UpdateVersion)` (compare-and-swap
against a known version), mapped per backend — `S3ConditionalPut::ETagMatch`
onto S3's native `If-Match`/`If-None-Match` (GA since August 2024, per doc 05),
and GCS's `ifGenerationMatch=0`/`=N` onto the same enum. This is exactly
`M1.5`'s `Precondition` mapping, already done at the client-library layer
rather than something `oqueue-store` has to hand-roll against two different
HTTP APIs. ⚠️ One caveat carried through from the underlying S3 API rather than
introduced by the crate: only the *finishing* call (`PutObject`/
`CompleteMultipartUpload`) can carry a conditional header — `UploadPart`
cannot — which is why `M1.16` conditions the multipart completion rather than
each part.

Built-in `RetryConfig`/`BackoffConfig` (exponential backoff with decorrelated
jitter) gives `M1.12` a policy to configure rather than a loop to write, and
built-in `put_multipart_opts` returning an explicit `MultipartUpload` handle
(`put_part()`, `complete()`, `abort()` — S3 and GCS do **not** auto-GC orphaned
parts) covers `M1.9`'s abstraction without `oqueue-store` reimplementing
multipart bookkeeping per cloud. ⚠️ **Correction, found implementing `M1.16`
(ADR-0013): this decision's own earlier claim — "only the *finishing* call
(`PutObject`/`CompleteMultipartUpload`) can carry a conditional header...
which is why `M1.16` conditions the multipart completion rather than each
part" — is wrong for `object_store`'s *public* API.** The crate supports a
conditional `CompleteMultipartUpload` internally but exposes it on neither
public multipart trait — see ADR-0013 for the full finding and what `M1.16`
actually ships instead.

`oqueue-store`'s existing manifest constraint (`check-layering.sh`: only
`oqueue-core` as a workspace dependency) is unaffected — `object_store` is an
external crate, not a workspace one, and appears only in `oqueue-store`'s
`[dependencies]`, never in `oqueue-core`.

## Alternatives considered

**Apache OpenDAL.** Broader backend coverage (50+ services against
`object_store`'s S3/GCS/Azure/local/memory) and a larger GitHub star count, but
two things it does not have: doc 05 could not confirm OpenDAL's conditional-
write behaviour is documented as precisely as `object_store`'s per-backend
`PutMode` mapping, and `object_store` is the crate `M1`'s risk section itself
already assumes (`milestones/M1.md`'s "current lean"). Breadth this project
does not need — no backend beyond S3, GCS and in-memory is on any roadmap
milestone — traded against a less-pinned-down story on the one property that
carries the most risk is the wrong trade here. The `object_store_opendal`
bridge crate keeps OpenDAL reachable later without a rewrite if a backend
outside `object_store`'s four is ever needed, so this is not a closed door.

**Provider SDKs directly** (`aws-sdk-s3` + `google-cloud-storage`). Full
fidelity to every cloud-specific feature — S3 Express One Zone, GCS `compose`
semantics exactly as documented — at the cost of two independent clients with
no shared trait, meaning `oqueue-store`'s S3 and GCS backends would each
invent their own shape and the seam's uniformity would be reconstructed by
hand inside this crate rather than inherited from underneath it. Rejected
because ADR-0005's whole premise — S3 and GCS behind one seam — is precisely
what the SDKs do not give for free, and this project's ecosystem-gravity
argument (below) also does not apply to them: they are single-cloud by
construction, so there is no shared dependency to inherit from other Rust
data-infra projects.

**Both — provider SDK for advanced features, `object_store` for the rest.**
Rejected as premature. Nothing on this milestone's task list ([Provisional
tasks](../milestones/M1.md)) needs an S3-Express-specific or GCS-specific
capability `object_store` lacks; `milestones/M1.md`'s decision #7 (whether the
storage tier stays pluggable) is explicitly deferred, blocked on NFR-13, and
this ADR does not need to answer it. If a future milestone needs a
cloud-specific feature `object_store` does not expose, that is a new decision
made with a concrete need in hand, not one to pre-empt here.

## Consequences

**Easy.** `M1.5`'s `Precondition` enum has a direct one-to-one mapping onto
`PutMode`, so it is a thin wrapper rather than new protocol logic. `M1.9`'s
multipart types wrap `MultipartUpload` rather than reimplementing session
bookkeeping. `M1.12`'s retry policy configures `RetryConfig` rather than
writing a backoff loop. The same crate DataFusion, delta-rs, Lance and Polars
already depend on (486 reverse dependencies per doc 05) means version bumps
and bug fixes in the object-storage layer are shared upstream work, not this
project's alone — relevant if a future milestone ever adds SQL-over-segments
or an Iceberg/Delta export, which would reuse the same trait rather than a
second storage abstraction.

**Hard.** `object_store`'s own `ObjectStore` trait is *not* reused directly —
`ADR-0005` already committed this project to its own `dyn`-compatible
`ObjectStore` trait with a hand-written boxed future, for reasons ADR-0002
recorded, so `oqueue-store`'s backends adapt `object_store`'s trait to this
project's rather than exposing it. `object_store`'s conditional-write support
is real but backend/configuration-dependent per doc 05 — an S3-compatible
store that cannot do atomic put-if-absent at all (unlike MinIO, which does)
would need its own finding, not assumed conformance from the crate choice
alone. And `object_store` v0.14.1's MSRV (1.85.0, per doc 05) is a floor
`Cargo.toml`'s `rust-version` must not go below once this dependency lands —
checked by `NFR-40`'s existing CI matrix, not a new gate.
