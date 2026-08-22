# 0014. GCS live-emulator verification is deferred, not built into `M1.17`

Status: accepted
Date: 2026-08-17
Requirements: FR-31

## Context

`testing.md`'s T2 tier names `fake-gcs-server` as the container `M1.17`'s GCS
backend should round-trip against — the GCS analogue of `M1.15`'s MinIO. Two
emulators were tried while building `M1.17`, both against the actual
`object_store` GCS client this crate depends on (not a hand-rolled request):

- **`fsouza/fake-gcs-server`** (the tool `testing.md` names). `object_store`'s
  GCS client `PUT`s directly to `<base_url>/<bucket>/<object>` — the XML API
  shape, confirmed against `object_store`'s own `gcp/client.rs` (`path_url`
  builds exactly that, no `uploadType` query parameter). `fake-gcs-server`
  routes that same path to its JSON-flavored `insertObject` handler
  (`fakestorage/server.go`'s route table, confirmed against its published
  source), which requires an `uploadType` query parameter or a signed-URL
  scheme and returns `400 invalid uploadType` otherwise
  (`fakestorage/upload.go`). Every `PUT`/`GET` this backend issues hits
  exactly this path.
- **Google's own `storage-testbench`** (`gcr.io/cloud-devrel-public-resources/storage-testbench`),
  tried as the more official alternative once the above failed. Its XML `PUT`
  route (`testbench/rest_server.py::xml_put_object`, confirmed against its
  published source) does store the object correctly — visible with the
  correct `ETag`/generation through its own JSON API — but the XML response
  it returns sets only an `x-goog-hash` header, never `ETag` or
  `x-goog-generation`. `object_store`'s `do_put` reads the response headers
  directly (`get_put_result(response, VERSION_HEADER)`) and fails with
  `Error::Metadata { source: MissingEtag }` before this crate's code is ever
  reached — confirmed by running the actual client against a live container,
  not inferred from source alone.

Both are real, reproducible gaps in third-party tooling, not a coding mistake
in `GcsStore`: `S3Store`'s equivalent MinIO round trip (`ADR` none needed —
it simply works) and `GcsStore`'s own logic are held to the identical
standard, and `GcsStore`'s decision logic (classification, precondition
encoding, multipart planning) is fully covered at T0 independent of either
emulator, the same way `S3Store`'s was before `M1.15`'s MinIO test existed.

`M1.md`'s own completion condition (⚠️ ~~the prose `scripts/gates/m1-complete.sh`
does not exist yet to enforce~~ — `M1.21` wrote that gate, and `M1.43` wired
the milestone-review check into it) names only the fake and MinIO — not a GCS
emulator — so this is not blocking `M1`'s own close, the same way real-S3
verification already isn't (`roadmap.md`'s deferred table, doc 10 #33).

## Decision

**`M1.17` ships `GcsStore` fully built and T0-tested, without a working T2
container test.** The conformance harness (`M1.10`) is not wired to a GCS
emulator in this commit — attempting it now would either paper over a broken
round trip with an `#[ignore]`d test that can never pass, or spend unbounded
time patching a third-party emulator's response-header behavior, neither of
which this task should do.

**`testing.md`'s T2 tier table is corrected**, not merely re-cited: it names
`fake-gcs-server` as workable, which this investigation found false for
`object_store`'s actual request shape. The table now says so and points here.

⚠️ **This paragraph's choice was correct when written and its outcome was
wrong; `M1.44` moved the deferral to `roadmap.md`'s cross-milestone table with
`M15` receiving.** `M1.21` closed with `gcs` recorded `not-yet-run`, which is
the honest matrix entry but leaves the obligation ownerless: the reasoning
below turns on the deferral not crossing a milestone boundary, and it crossed
one the moment M1's last row went `done`. The lesson is narrower than "use the
table always" — it is that **deferring to a task inside the current milestone
only holds if that task actually discharges it**, and nothing checks that,
which is why the table exists for the case where it does not.

~~**Live GCS verification is deferred to `M1.21`**, not a later milestone via
`roadmap.md`'s cross-milestone table — that table is for a deferral crossing
a milestone boundary, and `M1.21` ("conformance suite completion") is still
`M1`'s own task, the one that already planned to record the GCS row of the
backend matrix. `M1.21`'s own backlog row now carries this finding directly,
so it is not silently assumed there.~~

Whoever picks it up in `M15` has three
options this ADR does not choose between: get
`storage-testbench` to return the missing headers (file the gap upstream, or
find a still-unfound flag), get `fake-gcs-server` to accept the XML path (an
upstream feature request, since its route table simply does not have one),
or find a third emulator. Each is a real option; none was worth pursuing
further to close one backlog row.

## Alternatives considered

**Hand-write the missing response headers into the emulator's own request
path from `oqueue-store`'s side** (e.g. detect a GCS-shaped 200 with no
`ETag` and follow up with a `GET`/`HEAD` to recover the generation).
Rejected: this papers over a testbench-specific bug with backend-specific
code that would need to run against real GCS too (where the bug does not
exist), which is exactly the kind of environment-shaped special case
`build.md`/`error-handling.md`'s spirit argues against — and it would still
leave the actual conformance suite unverified against anything real.

**Block `M1.17` until a working emulator is found.** Rejected: `GcsStore`'s
own logic is correct and independently verified at T0 (the same standard
`S3Store`'s pure logic was held to before `M1.15`'s MinIO test existed), and
delaying real, complete, reviewed code behind a third-party tool with no
committed fix timeline blocks the milestone on something this project does
not control — the same reasoning ADR-0013 already gives for not blocking on
an upstream `object_store` fix.

## Consequences

**Easy.** `GcsStore` ships as real, exercised-at-T0 capability;
`crate::classify`/`crate::get`/`crate::multipart`'s sharing with `S3Store`
means most of its logic was already proven correct by `M1.15`/`M1.16`'s own
tests, and only the GCS-specific pieces (`version`-keyed preconditions,
GCS's own `MultipartLimits`) needed new coverage.

**Hard.** GCS's conditional-write behaviour — the single highest-risk
property in this project (`M1.md`'s own goal section) — stays verified only
against `object_store`'s source and this project's own unit tests, not
against any running GCS-shaped server, until whoever picks up the deferred
entry above closes that gap. This is a real, open risk, not a formality;
recorded here rather than left implicit is the point of writing this ADR at
all.
