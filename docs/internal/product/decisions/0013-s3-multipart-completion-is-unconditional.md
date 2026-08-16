# 0013. S3 multipart completion is unconditional in `M1`; conditioning it is deferred to `M3`

Status: accepted
Date: 2026-08-17
Requirements: FR-31

## Context

ADR-0008 (`M1.1`) picked `object_store` partly on the strength of one claim,
repeated in doc 05 §1 and doc 04 §5: that S3's conditional-write header can be
carried on *either* finishing call — `PutObject` **or** `CompleteMultipartUpload`
— with only `UploadPart` excluded. `M1.16`'s own backlog row assumed the same
thing: "conditional `PUT` maps `M1.5`'s `Precondition` to `object_store`'s
`PutMode`... and `CompleteMultipartUpload` honours the same `Precondition` as a
whole-object `put`."

That is true of **real S3's REST API** — confirmed independently by `object_store`
0.14.1's own source, which sends `If-None-Match: *` on `CompleteMultipartUpload`
today. It is not true of **`object_store`'s public Rust API**, which is what
`oqueue-store` actually depends on. Traced in full while implementing `M1.16`:

- `aws/client.rs` defines `pub(crate) enum CompleteMultipartMode { Overwrite,
  Create }`, where `Create` sends `If-None-Match: *`. It is `pub(crate)` —
  unreachable from `oqueue-store` by name.
- The only place `CompleteMultipartMode::Create` is ever constructed is inside
  `AmazonS3::copy_opts`'s `CopyMode::Create` arm, as an implementation detail of
  copy-if-not-exists (via `S3CopyIfNotExists::Multipart`) — not exposed for a
  general multipart *upload*.
- Both public entry points a caller can actually reach —
  `MultipartUpload::complete()` (via `put_multipart_opts`) and
  `MultipartStore::complete_multipart()` — hardcode
  `CompleteMultipartMode::Overwrite` in their `AmazonS3` implementations, with no
  parameter, extension point, or `PutMultipartOptions` field that reaches it.

This is a known, open upstream gap, not a misreading:
[`apache/arrow-rs-object-store#289`](https://github.com/apache/arrow-rs-object-store/issues/289)
("`put_multipart_opts` should allow setting `PutMode`"), filed 2025-02-10, still
open as of this ADR, and its own description matches this investigation almost
exactly: *"Support for this is already partially in place, `S3Client`'s
`complete_multipart` accepts a `CompleteMultipartMode`, but this is currently
only used to implement `copy_if_not_exists` and not for normal multipart object
uploads."* 0.14.1 is the newest published version (checked against the crates.io
index directly); there is no newer release that might already have fixed it.

**A second, independent problem, found while scoping what `M1.16` could still
build**: doc 04 §5's actual motivating scenario for multipart — "a log-storage
layer accumulating a segment can open a multipart upload, flush accumulated
buffer chunks as parts as they fill... and only 'seal' the object... once the
segment is closed, without ever needing to know the final size up front" — is a
**streaming** writer. `oqueue-core::ObjectStore::put(key, payload: Vec<u8>,
precondition)` cannot express that at all: the whole payload is already in
memory by the time `put` is called, which is the opposite of "unknown final
size." A real streaming-multipart seam needs a *new* trait capability —
`contracts.md` rule 12's territory, an ADR and every implementor updated in one
commit (non-negotiable 6) — which is too large to fold into a task titled "S3
multipart" and premature before any milestone has a caller that needs it. `M1`
has none: nothing before `M3` assembles a growing object at all (`M1.7`'s own
finding, carried in `roadmap.md`'s deferred table for the region-header `alg`
field, for the identical reason).

## Decision

**Two separable things, decided differently.**

1. **`S3Store::put` gains real multipart support, unconditional only.** A
   payload whose size exceeds `MultipartLimits::max_part_size` (S3's real
   numbers: 5 MiB minimum part except the last, 5 GiB maximum part — not
   coincidentally the same as the maximum a single non-multipart `PutObject`
   may ever carry — 10,000 maximum parts, 5 TiB maximum object) is split into
   parts and uploaded via `object_store`'s public `MultipartUpload` API,
   validated against `M1.9`'s `MultipartSession` before any request is sent.
   `precondition: Some(_)` combined with a payload that size returns
   `Error::Permanent` — the exact combination this backend cannot currently
   honour, and retrying it with the same oversized, conditioned payload will
   never succeed, matching `Permanent`'s own contract. This is real,
   exercised capability: it handles anything this trait's `put` can be asked
   for **except** "condition a write bigger than 5 GiB," which is not a shape
   this project's own write pattern (`M1.20`'s 4 MiB-aligned chunks) is
   expected to ever produce.
2. **The streaming-writer-with-conditional-seal capability doc 04 §5 actually
   motivates multipart with is deferred to `M3`** — `roadmap.md`'s "Deferred
   into a later milestone" table and `M3.md`'s own plan, mirroring the
   region-header `alg` field's identical reasoning. `M3` is where a new seam
   capability for it gets designed, at the point something actually needs to
   call it, against whatever `object_store`'s public API looks like by then.

## Alternatives considered

**Hand-roll a raw, SigV4-signed `CompleteMultipartUpload` request**, bypassing
`object_store` for just this one call. Rejected: this is exactly the cost
ADR-0008's Context section names as the reason to prefer a client library over
provider SDKs or hand-rolled protocol code — "a correctness-critical property
depending on [conditional writes]" pushed into "hand-rolled per-backend header
code this project would then have to get right itself." Credential resolution,
request signing, and retry/backoff would all need reimplementing or duplicating
from what `object_store`'s builder already does internally, for one call, with
its own security review burden this project's `security.md` would need to hold
it to.

**Intercept the outgoing HTTP request via a custom `object_store::client::http::HttpConnector`/`HttpService`**,
injecting `If-None-Match`/`If-Match` into whichever request matches
`CompleteMultipartUpload`'s shape (a `POST` with `uploadId` in its query
string) before it reaches the network. Technically reachable — both traits are
`pub`. Rejected anyway: it depends on an unverified assumption (that S3's
SigV4 signature verification does not require these headers to be part of the
signed set, which nothing in `object_store`'s public documentation confirms
either way), matches requests by brittle URL-shape sniffing rather than any
contract `object_store` documents as stable, and sits underneath a library
specifically to defeat a choice its own maintainers made deliberately —
exactly the kind of code a future maintainer cannot safely reason about
without re-deriving this whole investigation. If `M3` ever needs this badly
enough to justify the risk, that is a decision for `M3` to make with a
concrete caller in hand, not one to pre-empt here.

**Wait for upstream `object_store` to close #289** and pin whatever version
fixes it. Not rejected outright — genuinely worth revisiting — but not
something this ADR can decide today: there is no committed timeline, and `M1`
cannot block its own completion condition on an issue this project does not
control. `M3` re-checks this before choosing one of the other two paths.

## Consequences

**Easy.** `M1.16` ships real multipart support for what it can actually
express today, exercised by the conformance-style tests it adds, not left
half-built or silently dropped. `M1.9`'s `MultipartLimits`/`MultipartSession`
get their first real backend consumer, proving the abstraction those pure
types encode actually fits a live request against S3.

**Hard.** `M3`'s plan must not repeat ADR-0008's mistake and assume
`object_store` already solved conditional multipart completion — it starts
from this ADR's finding, not from doc 04 §5's original, now-corrected claim.
Doc 04 §5 and doc 05 §1 both carry a correction note pointing here rather than
silently continuing to assert something this investigation found false.
