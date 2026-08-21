---
title: "Backlog"
description: >
  Read when picking up work. Only the current milestone is decomposed.
tags: [product, tasks, planning]
---

# Backlog

One task equals one commit equals one change that leaves the tree green. If a
task cannot be finished in one green commit, split it before writing code.

Task IDs are stable. Completed tasks stay here with their commit reference so
the history of why something was done survives.

**The current milestone's section is first; finished milestones follow it, most
recent first.** ⚠️ Written down rather than left to convention, because
`scripts/lib.sh` deliberately does *not* derive the current milestone from this
file's headings — it derives it from the commits, and its own comment says
reading the last heading instead "would only trade that for a dependency on an
unwritten ordering convention". Nothing reads the order; `next-task` reads "the
top unblocked task" and a human reads the top of the file, and both want the
milestone actually being built.

Decomposition rule: only the current milestone is decomposed in detail. Future
milestones stay as [roadmap](roadmap.md) entries with a
[plan](milestones/README.md) until their turn. ⚠️ **A plan is not a
decomposition** — it is a hypothesis, it carries no task IDs, and its items are
re-derived rather than copied when a milestone opens. Read the plan's
"Decisions required first" before writing any of that milestone's code; see
[`sdd.md`](../standards/sdd.md) §Decomposition.

## M1: Object store seam and conformance suite

`ObjectStore` and its three implementations — in-memory, S3, GCS — plus the
backend-agnostic conformance suite that decides whether the fake can be
trusted. No broker code: this crate is deliberately buildable and testable
without one. Plan: [milestones/M1.md](milestones/M1.md).

⚠️ **M1 carries 42 rows, over `sdd.md`'s cap of 20, and here is the argument
the standard requires.** The milestone is named *Object store seam and
conformance suite*: `M1.1`-`M1.21` are that seam — two ADRs it needs before any
code lands, three real implementations (fake, S3, GCS) sharing one error
taxonomy, one precondition mapping, one retry policy, one rate governor, one
key layout and one conformance suite. None of that is a
second milestone wearing this name — it is the actual size of "three backends
behind one seam, proven equivalent by one suite". `M1.22` is the madsim spike
the plan itself carries as non-code, so it costs a row without costing scope.
`M1.33` through `M1.41` are findings review made after this
decomposition was written — a doc-04 citation that does not support what it was
cited for, a CI job that turned out never to have existed, a name-based
threshold matcher whose backstop covers three hardcoded constants, a gate
label pinned by accident, a crate documenting a backend it does not have, a universal claim that outlived its own correction, a wiring comment two backends have overtaken, a budget exemption whose frequency needs a policy before it becomes the norm, and a standard that routed to neither the crate holding every secret nor its own path — so they cost rows without costing scope either. ⚠️ **This number is prose about a table, which is the
shape this repository keeps finding stale**: it said 33 while the table held
35. Recount before trusting it: `grep -cE '^\| M1\.[0-9]+ \|'` on this file.
`M1.23`-`M1.32` are not new scope either: they are the ten concrete,
reproducible findings M0's boundary reviews recorded and did not fix —
`milestones/M1.md`'s own "Inherited from M0's boundary review" section —
and `sdd.md`'s third-bucket reasoning from M0's own decomposition note applies
here unchanged: a milestone's own inherited claims-not-yet-true are a reason to
carry rows, not the growth the cap exists to stop. ⚠️ **That reasoning has a
limit and this paragraph is not licence to reuse it indefinitely.** If M1's own
boundary review turns up a further crop of undone findings, those are M2's to
carry, the same way M0 stopped absorbing its own residue at its closing review
rather than inventing a further bucket; `roadmap.md`'s deferred-procedure row
is where the unbounded version of this problem is tracked, not here.

⚠️ **Two decisions gate this milestone's code, and both are ADR tasks —
`M1.1` and `M1.2`.** `milestones/M1.md`'s "Decisions required first" names a
third, decision #7 (whether the storage tier stays pluggable): it is **not** a
task here, because it is blocked on NFR-13, which carries no number yet
(`roadmap.md`'s "numbers that do not exist yet" table) — deciding it now would
mean inventing the number `sdd.md` forbids inventing. What this milestone
builds must not foreclose the answer either way; no task below does.

⚠️ **The plan was an input, not this list.** Eighteen provisional tasks became
nineteen (one merge, two splits below), the four "decisions required" became two
ADR tasks and one deliberately-not-a-task, and the madsim spike stayed
non-code. `M1.23`-`M1.32` have no plan item at all — they come from
`milestones/M1.md`'s inherited-findings section, not from its provisional-task
list. "Where this decomposition diverged from the plan" below carries the
item-by-item mapping.

| ID | Task | Acceptance | State |
| --- | --- | --- | --- |
| M1.0 | Open M1: decompose it here, flip the State cells in `roadmap.md` and `README.md` (`M0` → complete, `M1` → in progress) and correct the one sentence in `README.md`'s prose that still calls M0 "in progress", and record where this decomposition diverges from the plan | `scripts/check-requirements-trace.sh` passes with `milestones/M1.md`'s `**Serves:**` line and `roadmap.md`'s requirement-coverage row agreeing (already true — FR-30, FR-31, NFR-30 on both); every row `M1.1`-`M1.21` names at least one of FR-30, FR-31 or NFR-30; `M1.7` (a field pulled forward from M8's FR-41-43, not one of M1's own) and `M1.22`-`M1.32` (the spike and the inherited gate/documentation fixes) name none, each with an explicit note saying so — the same shape as `M0`'s own gate-tail rows; `roadmap.md`, `README.md` all read `M0` complete and `M1` in progress; `scripts/build-index.sh --check` and `scripts/check-portability.sh` pass. ⚠️ Serves no FR/NFR itself, for the same reason `M0.0` did not | done |
| M1.1 | **ADR-0008**: `object_store` vs `opendal` vs provider SDKs (decision #3) | Serves FR-31. The ADR names the crate `oqueue-store`'s backends are built on; states the alternatives doc 05 §1 lists and why each is rejected; and states explicitly whether the choice supports native `PutMode`/precondition CAS on both S3 and GCS, since `M1.5`'s precondition mapping depends on the answer. No code changes outside `Cargo.toml`'s `[workspace.dependencies]` gaining the chosen crate (pinned, no default features that pull a C toolchain — `build.md`'s dependency rule) | done |
| M1.2 | **ADR-0009**: whose enum holds `ObjectStore`'s errors, and whether `list()` belongs on the trait | Serves FR-31, NFR-30. Resolves the gap M0's boundary review found (`a4d96f6ba946`): states, with reasons, which crate's enum carries the six variants `M1.4` names, per `contracts.md` rule 17 and `error-handling.md` rules 3/7-8; and decides `list()`'s placement per doc 12 §8 — on `ObjectStore` or on a separate `MaintenanceStore` seam so "never LIST on the read path" (NFR-30) is checkable at compile time rather than only by a runtime gate. `architecture.md` gains a line recording whichever seam split is chosen. No code beyond what the ADR needs to state precisely. ⚠️ **This row originally named `oqueue-store::Error` as the expected answer**, matching finding `a4d96f6ba946`'s own framing; the ADR decides `oqueue-core::Error` instead, once the fake's placement (`contracts.md` rule 9, beside the trait, in `oqueue-core`) is reconciled against `oqueue-core`'s zero-dependency rule and ADR-0005's one-concrete-type-at-startup commitment — a constraint the finding's evidence did not have to weigh against. Corrected here per `sdd.md`'s "when the spec turns out to be wrong" rather than left standing against what the ADR actually says | done |
| M1.3 | `ObjectStore` trait + core types, replacing `oqueue-core`'s M0 placeholder trait | Serves FR-30, FR-31. `put`, `get(ByteRange)`, `delete(set)` on the trait, in `oqueue-core` per `ADR-0009`; `ObjectKey` (already existed), `ByteRange`, `ObjectMeta`, and an opaque `PreconditionToken` — not comparable across backends, not a content hash (multipart/SSE-KMS break that). Per `contracts.md` rule 12 this is a `pub trait` method-set change, so it carries its own ADR (**ADR-0010**) and updates `FakeObjectStore`, the trait's one implementor, in the same commit. `check-sans-io.sh`, `check-layering.sh` and `check-core-contract.sh` pass; `scripts/check-crate.sh oqueue-core` green (this milestone's code so far is entirely in `oqueue-core` — `oqueue-store` gains no code until `M1.15`/`M1.17` wire a backend against this trait) | done |
| M1.4 | Error taxonomy and classification | ⚠️ **Dissolved into `M1.6`, `M1.8` and `M1.15`, discovered before any commit touched it.** `error-handling.md` rule 6: "a variant nothing can produce is not added 'for completeness' — an error type is a claim about what can go wrong, and a variant nothing can produce is a claim that is false the day it is written." `NotFound` already exists (`M0`). `PreconditionFailed` has no producer until conditional writes exist, which is `M1.6`'s own row already, word for word ("conditional writes obey `Precondition` and return `PreconditionFailed` on a losing race") — so a standalone `M1.4` adding it first would sit unproducing for one task's distance, and testing it would mean fabricating a fake failure with no real path to it. `SlowDown`, `Throttled`, `Transient`, `Permanent` have no producer until a real backend exists to classify into them — eleven tasks away at `M1.15` — so adding them now and writing "a unit test constructs each backend-shaped raw error" would mean the test invents the shape of an error `object_store` has not been asked to produce yet, which is coverage theatre against a shape nobody chose. ⚠️ **`M1.8`'s own row revised this further**: fault injection on the fake is itself a legitimate near-term producer of `SlowDown`/`Throttled`/`Transient` — the fake, under an explicit knob, *becomes* the backend for the purpose of simulating exactly these classes — so those three move to `M1.8` rather than waiting the full eleven tasks for `M1.15`. Moved: `M1.6`'s row gains `PreconditionFailed`; `M1.8`'s gains `SlowDown`, `Throttled`, `Transient`; `M1.15`'s keeps only `Permanent` (no fault-injection knob simulates it — it does not fit "storm") plus the classification/mapping layer from `object_store::Error` — each landing beside the first code that actually produces it | dissolved |
| M1.5 | `Precondition` enum | Serves FR-31. ⚠️ **Narrowed before implementation, spec-check step of picking this task up**: the enum itself (`IfAbsent` / `IfMatches(PreconditionToken)`) is backend-agnostic vocabulary and belongs in `oqueue-core` beside `ObjectStore`, but a type literally named `S3Condition`/`GcsCondition` rendering it to `If-None-Match: *` or `ifGenerationMatch=N` would name a backend in the one crate NFR-51 forbids that in — so the actual S3/GCS encoding, and the round-trip test the original wording named, move to `M1.15`/`M1.17`, the first code with a real header or query parameter to round-trip against. What lands here: the enum, and a test that an unrepresentable combination (e.g. "absent and matches some token") is a compile-time impossibility — no variant expresses it — not a runtime check that could be skipped. ⚠️ **The state cell said `todo` until M1's rows were audited before the milestone review**: commit `4768ea0` rewrote this row's description to record the narrowing and never flipped the cell beside it, so a completed task stayed on the list `next-task` reads — and ADR-0011 and ADR-0013 already referred to this `Precondition` as existing. The work landed in that commit: `precondition.rs` plus `tests/it/precondition.rs`, two tests, and a non-wildcard `match` over the two variants. ⚠️ That `match` is a canary rather than the proof — what makes "absent *and* matches" unrepresentable is the enum being a sum type; the `match` is what stops a third variant being added *silently*, since one would break the build here first. ⚠️ **The audit's method is weaker than the defect it swept for**, and saying so matters if it is reused: grepping commit subjects for a task ID tests authorship, not whether work shipped, and work can land under a neighbouring ID — `M1.4`'s dissolution landed inside this very commit. Review closed that gap by content-checking all nine open rows (none has shipped) and by replaying every state-cell transition across the history for the **inverse** defect — a `done` cell whose work never landed — finding none: every other row was flipped by the commit naming it, with two self-declared exceptions | done |
| M1.6 | Conditional writes on the fake | Serves FR-31. ⚠️ **Corrected before implementation** — the original wording said `FakeObjectStore` "is deleted or reduced to a re-export", which contradicts `contracts.md` rule 9 (the fake lives beside the trait, in `oqueue-core`, always); `M1.3` already rewrote it in place rather than adding a second one beside it, which is what "replacing M0's placeholder rather than sitting beside it" actually meant, and that work is done. What is left: `put` gains an `Option<Precondition>` parameter (`None` keeps today's unconditional overwrite) — another `contracts.md` rule-12 change, its own ADR (**ADR-0011**). `Some(IfAbsent)` fails `PreconditionFailed` if a payload currently exists; `Some(IfMatches(token))` fails `PreconditionFailed` unless the key's current token matches, checked and written atomically under the one lock the fake already holds. `Error::PreconditionFailed` (dissolved in from `M1.4`) lands in this commit, the first thing to produce it. A concurrency test proves last-writer-wins on two unconditioned racing `put`s: exactly one payload survives, never a mix. `check-core-contract.sh` passes | done |
| M1.7 | Region header's `alg` field | ⚠️ **Dissolved into `M3.19`, discovered before any commit touched it.** The task assumed a "region header/footer type" already exists in `M1` for a field to land on — none does. `oqueue-store` (this milestone's only crate with new code) stores opaque `Vec<u8>` payloads and is architecturally required to stay unaware of any structure within them (`architecture.md`'s Encryption section: "`oqueue-store` stays unaware that bytes are encrypted"). The object's internal structure — where a region header actually lives — is first defined by `M3`'s multi-topic flush batching (FR-32), the first milestone that assembles a bundled object. Doc 10 #40 itself corrected (2026-08-16) from "the region header must name its algorithm from M1" to "from its first commit"; `roadmap.md`'s deferred-into-a-later-milestone table, `milestones/M1.md` and `milestones/M3.md`'s task 19 all updated to match. The timing constraint doc 10 #40 cares about — the field exists before any region header is ever written, not retrofitted — is unchanged; only which milestone's first commit satisfies it moved | dissolved |
| M1.8 | Fault injection on the fake | Serves FR-31. Latency distributions, 503-storm injection, and crash-after-`PUT`-before-commit each land behind an explicit `FaultConfig` knob (not always-on) — a test proves each fault mode is reachable and that the fake with all knobs off is unchanged from `M1.6`'s behaviour. ⚠️ **Conditional-write race interleavings — narrowed, not a knob.** A callback-style pause hook between check and commit would need to run inside the fake's own lock acquisition, and the only caller-supplied code that could run there is a test's, on the *same* thread that already holds the lock — `std::sync::Mutex` is not reentrant, so that shape deadlocks by construction the first time a test tries it. Exercised instead the way `M1.6`'s unconditioned race already was: real OS threads racing genuinely conditioned `put`s against the atomic check-and-write `M1.6` built, proving exactly one racer can hold a given precondition. **Also carries three of `M1.4`'s dissolved variants**: `Error::SlowDown`, `Throttled` and `Transient` land here — the storm knob is their first real producer, since simulating backend unavailability is exactly what they classify. `Permanent` stays with `M1.15`; nothing about a fault-injection storm resembles an unretryable client error | done |
| M1.9 | Multipart/resumable abstraction types | Serves FR-31. ⚠️ **Bounds are configurable, not hardcoded to S3's numbers** — S3 and GCS do not share a shape (S3: part size + part count; GCS: 256 KiB chunk multiples, no part-count analogue), and naming S3's specific numbers as a constant in `oqueue-core` would violate NFR-51. `MultipartLimits { min_part_size, max_part_size, max_parts, max_object_size }` plus `MultipartSession::add_part`/`finish`, each bound rejected at the point it becomes knowable (min-part-size only at `finish`, since a part below it is legal if it turns out to be last) rather than in a comment. Unit tests exercise each of the four bounds at one under, one at, one over, plus a realistic run using S3's own documented numbers (5 MiB min except last, 5 GiB max part, 10,000 parts, 5 TiB object — doc 04 §5) as test data, not as an exported constant | done |
| M1.10 | Conformance harness + capability matrix, run against the fake | Serves FR-31 — its own verification method ("conformance suite run against every backend"). One parameterized test body executed once per registered backend; a recorded "which backends has this run against" artifact; a capability matrix so a backend can declare an operation unsupported and have the suite skip (and record) rather than silently pass. Running it now registers only the fake; `check-crate.sh oqueue-store` green with the harness in the suite | done |
| M1.11 | Key layout: computable, offset-aligned keys with hash fan-out | Serves FR-31. A pure function from `(topic, partition, offset-or-generation)` to `ObjectKey`, with no dependence on lexicographic order (S3 Express directory buckets do not preserve it) — a property test asserts fan-out distributes across the configured prefix count and that no two logically distinct inputs collide | done |
| M1.12 | Retry policy driven by error class | Serves FR-31. Retries `Transient`/`SlowDown`/`Throttled` with backoff; **never** retries `PreconditionFailed` — a test proves a `412`-classified error reaches the caller on the first attempt, and that retrying it would have silently converted a lost CAS to last-writer-wins is stated in the doc comment, not just tested | done |
| M1.13 | Per-prefix request-rate governor | Serves FR-31. Sized to S3's 3,500 write / 5,500 read per second per prefix, and GCS's ramp (roughly doubling every 20 minutes) as a distinct, swappable policy — a test using a fake clock proves the governor admits at the configured ceiling and rejects or delays above it, with no `sleep` (`tdd.md`'s no-flake rule) | done |
| M1.14 | Op-class accounting | Serves FR-31. ⚠️ **`Operation` has exactly `Get`/`Put`/`Delete` — `ObjectStore`'s three actual methods, no more.** `list()` did not survive `M1.2`'s ADR-0009 (permanently absent, not merely undecided when this row was first drafted), so there is nothing named `List` to count; `MultipartUploadPart` is deferred to `M1.16`, the first commit that issues one, per `error-handling.md` rule 6's reasoning applied to a counter rather than an error variant. `CountingObjectStore<S>` wraps any `ObjectStore` (composes with the fake now, a real backend once `M1.15`/`M1.17` build one) and increments on invocation, not on success — a failed call still cost an API request. A test drives a sequence of calls through a wrapped fake and asserts the counts, making cost a property the conformance suite (and later NFR-31's cost model, M14) can assert rather than a review convention | done |
| M1.15 | S3 backend: core ops + conditional `PUT` | Serves FR-30, FR-31. `get`/`put`/`delete` against an S3-compatible endpoint using the crate `ADR-0008` chose; conditional `PUT` maps `M1.5`'s `Precondition` to `object_store`'s `PutMode` — this is where the S3 side of `M1.5`'s deferred round-trip test lands, since this is the first code with a real S3 conditional-write encoding to round-trip against; the conformance harness (`M1.10`) registers it and passes against MinIO in CI. Credentials come from the environment, never a literal in code or test fixture (`security.md`). **Also carries `M1.4`'s last remaining dissolved variant**: `Error::Permanent` lands in this commit, the first thing able to produce it (`SlowDown`/`Throttled`/`Transient` moved to `M1.8`'s fault-injection storm instead), plus the classification/mapping layer from `object_store::Error` into all four — a unit test constructs each `object_store`-shaped raw error this backend can actually receive and asserts the classified variant, rather than a shape invented ahead of a real producer. ⚠️ **Also settled the TLS crypto-provider question ADR-0008 deferred to this commit**: `ring`, not `aws-lc-rs` (**ADR-0012**), reached through a feature-only `reqwest`/`rustls` dependency pair plus a runtime `install_ring_provider` call — `object_store`'s and `reqwest`'s own Cargo features cannot express "TLS with `ring`" without also pulling in `aws-lc-rs` somewhere in the graph; the ADR records the mechanism, confirmed by `cargo tree -i aws-lc-sys`/`-i aws-lc-rs` finding neither package and by a real HTTPS round trip under the installed provider. ⚠️ **One documented gap, found writing the conformance test against real MinIO**: a ranged `get` whose *end* runs past the object's true size is checked against `GetResult::range` and correctly raised as `Error::ByteRangeOutOfBounds`, but a range whose *start* is at or past the true size is rejected by `object_store` itself as an unclassifiable `Generic` error before this backend ever sees a size to compare against — it surfaces as `Error::Transient` instead. Closing that needs either an extra `HEAD` per ranged `get` or reaching into `object_store`'s own `pub(crate)` error internals (`error-handling.md` rule 8 already rules out the latter); left as `s3.rs`'s own documented limitation rather than spending `M1.19`'s scope early | done |
| M1.16 | S3 multipart, unconditional | Serves FR-31. ⚠️ **Narrowed before implementation, spec-check step of picking this task up — see ADR-0013.** The row as originally written ("`CompleteMultipartUpload` honours the same `Precondition` as a whole-object `put`") assumed `object_store` exposes a conditional multipart completion publicly; it does not (`CompleteMultipartMode::Create` is `pub(crate)`, used only by the crate's own copy-if-not-exists, not reachable from `oqueue-store` — confirmed against the source and against the open upstream issue `apache/arrow-rs-object-store#289`). What lands here instead: `S3Store::put` splits a payload larger than `MultipartLimits::max_part_size` (S3's real numbers: 5 MiB min part except last, 5 GiB max part, 10,000 max parts, 5 TiB max object) into parts and uploads them via `object_store`'s public `MultipartUpload` API, validated against `M1.9`'s `MultipartSession` before any request is sent — real, exercised capability using `M1.9`'s types, per the original row's intent. `precondition: Some(_)` combined with a payload that size returns `Error::Permanent` rather than silently dropping the precondition. The streaming-writer-with-conditional-seal capability doc 04 §5 actually motivates multipart with is deferred to `M3` (`roadmap.md`'s deferred table, `M3.md` task 19) — it needs a new seam capability `ObjectStore::put`'s complete-in-memory-payload shape cannot express regardless of the `object_store` gap, which is too large for this task and premature before `M3`'s flush batching gives it a real caller. ⚠️ **Review found a second deferral this row's own text had not named**: `M1.14`'s `Operation`/`CountingObjectStore` count one call per `put`, undercounting a multipart `put`'s real request cost (`CreateMultipartUpload` + N `UploadPart`s + `CompleteMultipartUpload`, all one counted `Put` from a decorator wrapping the trait) — `M1.14`'s own comment had assumed this commit would just add a variant, which turned out not to be enough; the actual fix needs a design choice (a contract change, or per-backend accounting) that belongs to `M14`'s API-cost model (NFR-31), not this row — `roadmap.md`'s deferred table, `M14.md` task 5 | done |
| M1.17 | GCS backend | Serves FR-31. ⚠️ **Narrowed before implementation, spec-check step of picking this task up — see ADR-0014.** `GcsStore` implements `get`/`put`/`delete`/multipart via `object_store`'s GCS client (`gcp-base` feature, same `ring`-not-`aws-lc-rs` mechanism ADR-0012/`M1.15` already established); conditional writes map `M1.5`'s `Precondition` to `ifGenerationMatch=0`/`=N` through `PutMode`, encoded via `UpdateVersion::version` rather than `::e_tag` — the one genuinely GCS-specific piece, confirmed against `object_store`'s `gcp/client.rs`; `AlreadyExists`/`Precondition` classification, `get`'s range logic, and the size-based multipart decision are shared with `S3Store` (`crate::classify`/`crate::get`/`crate::multipart`) since none of that logic is actually backend-specific. Multipart completion is unconditional-only, same finding and same ADR-0013 reasoning as S3's — confirmed independently for GCS (`gcp/mod.rs`'s `MultipartStore::complete_multipart` also takes no precondition). **What the row as originally written assumed — "the conformance harness registers it and passes against a GCS emulator" — did not hold**: neither `fake-gcs-server` (`testing.md`'s named tool) nor Google's own `storage-testbench` round-trips `object_store`'s actual GCS requests out of the box (ADR-0014 has the full trace). `GcsStore`'s own logic is fully covered at T0 instead; live emulator verification is `M1.21`'s to pick up, per that row's own new note. `compose` is **not** built here — `M1.18`'s row already owns GCS `compose` under `copy_range`, and this row's own earlier mention of it was carried over from the shared plan text rather than a real second scope | done |
| M1.18 | `copy_range` server-side (S3 `UploadPartCopy`, GCS `compose`) | ⚠️ **Dissolved into `M5` task 7, discovered before any code touched it — see ADR-0015.** The row assumed `object_store` exposes server-side range copy for at least one backend; it exposes it for **neither**, confirmed exhaustively: S3's `UploadPartCopy` support is `pub(crate)`-only and whole-object-only even internally (no `x-amz-copy-source-range` anywhere in the crate); GCS `compose` does not exist in `object_store` at all. Both are tracked upstream as one gap — `apache/arrow-rs-object-store#121`, open since 2023-10-20, naming S3/GCS/Azure composition together. There is no partial version buildable through the crate `ADR-0008` chose; the only path is hand-rolled signed HTTP requests, the same infrastructure ADR-0013 already declined to build for a narrower problem. `M5.md` task 7 is the one real consumer and already names this capability, so it moves there — decided with a real caller and a real cost model, not projected ahead of one (`error-handling.md` rule 6's reasoning applied to a capability instead of an error variant) | dissolved |
| M1.19 | Ranged-GET read path: request merging + sparsity gate | Serves NFR-30, FR-31. ⚠️ **Narrowed before implementation, spec-check step of picking this task up — see `merge.rs`'s own module doc.** The row as originally written ("calls in flight are merged") describes coalescing ranged-`get`s arriving independently over time from concurrent callers, which needs either a real clock (a debounce window to let concurrent arrivals accumulate before dispatching — a sans-io violation, NFR-51) or an explicit multi-caller synchronization barrier with no natural trigger when callers arrive asynchronously and independently. What lands instead: `MergingObjectStore::get_many` takes a **batch** of ranges the caller already knows it wants together — the shape a Fetch response actually has once `M2` exists — and merges that batch's `Bounded` ranges by gap, purely and synchronously, before issuing any backend call; `Full` ranges never merge (no numeric span to merge by ahead of the fetch that would report the object's size) and always cost their own call. Lives in `oqueue-core` beside `CountingObjectStore`, not `oqueue-store` — no clock, no cross-task coordination, no new dependency, composes with `CountingObjectStore` in either order. `Error` gains `Clone` in this commit (`error.rs`'s own doc comment explains why): a merged group's single failure must reach every one of its members as an independent `Err`, not one value several callers would otherwise have to share. A test with `CountingObjectStore` wrapping the fake proves two adjacent ranges cost one backend call and two ranges past the threshold cost two, both counted, not inferred | done |
| M1.20 | 4 MiB-aligned chunk addressing + single-flight | Serves NFR-30, FR-31. Reads are addressed in 4 MiB-aligned chunks; concurrent readers requesting the same chunk share one in-flight backend request — a test with 500 concurrent readers against a call-counting fake asserts far fewer than 500 GETs reach the backend. `ChunkedObjectStore::get_chunk` lives in `oqueue-core` beside `CountingObjectStore` and `MergingObjectStore`, not `oqueue-store` — the same reasoning as both: no clock, no new dependency, forwards `get`/`put`/`delete` unchanged so it composes with either decorator in any order. Single-flight is a hand-rolled `Future` (`Join`/`LeaderGuard`, `ADR-0002`'s no-async-runtime posture applied to genuine cross-task coordination rather than `M1.19`'s synchronous batch merge): the first caller for a given `(key, chunk_index, length)` becomes the leader and issues the real fetch, every concurrent caller after it joins a shared, mutex-guarded result cell and is woken once the leader publishes — including on cancellation (`LeaderGuard`'s `Drop` publishes `Err(Transient)` rather than stranding followers `Pending` forever). `length` is part of the join key, not assumed to equal `chunk_size`, because the object's final chunk is legitimately shorter and two callers naming the same index with different lengths are not asking for the same bytes. ⚠️ **Independent review caught that `length` was then validated by nothing**, which is worse than it first sounds: an unchecked length does not merely let a 4 MiB-chunk store issue a 1 GiB `GET`, it silently partitions callers of the *same* chunk into groups that never share a fetch — single-flight degrading to no dedup at all, with every test still green, because each test passed a literal. It now fails at the door, before the join key is built: `Error::ChunkLengthTooLarge` (new variant, real producer, classified `RetryClass::Never` — `retry.rs`'s exhaustive match is what forced that to be an explicit decision) above `chunk_size`, and the existing `Error::EmptyByteRange` at zero. Both are asserted to cost no backend call. ⚠️ This is a **door check and weaker than the crate's own standard**: `lib.rs`'s "unconstructible-around" property is explicitly "not merely checked at the door", which a plain `u64` parameter cannot have. Two callers naming the same chunk with *different but individually legal* lengths still do not share a fetch, and only the object's size could say which is right — which this type deliberately does not know. ⚠️ **The acceptance test went through two wrong shapes before the right one, and each gate caught the next problem.** First shape: 500 real OS threads. Flaky — `FakeObjectStore::get` resolves on its first poll with no latency configured, so a leader's entire `get_chunk` (dispatch, finish, deregister) could complete before the second of 500 sequentially-spawned threads was even scheduled, reproducing intermittently as 500/500 backend calls with zero dedup. Caught only by running it repeatedly rather than trusting one green run (rule 3's discipline extended to racy tests). Second shape: same threads plus a `Barrier` and injected latency, stable across 20+ runs — but **`check-mutants.sh` refused it**, reporting `LeaderGuard::publish` and `get_chunk`'s `!is_leader` as **timeouts rather than caught**. The tests did detect both mutants: by hanging, because the test-local `block_on` was an unbounded busy-spin and a follower whose leader never publishes spins forever. A suite that detects a defect by never finishing is strictly worse than one that fails, and it was degrading `testing.md` rule 15's primary gate into a stalled run. A `Barrier` only makes a race *likely* to go the right way, never certain. Third and shipped shape: a cooperative executor (`run_all`, in its own `#[cfg(test)] mod test_executor` — `check-file-size.sh` pushed it out of `chunk.rs`, and rule 18 was right both times it fired here that the split is by concept: an executor is not chunk addressing, and the single-flight machinery — `ChunkKey`/`Shared`/`Join`/`LeaderGuard`, now `chunk/single_flight.rs` — is not the chunk addressing that uses it) polling 500 futures on one thread via a real ready-queue `Waker`, which **detects deadlock instead of hanging** — nothing runnable while futures are still pending is a bug, and it panics on it in microseconds. The interleaving becomes a fact rather than a hope (`latency_polls: 1` suffices, since the batch is polled before returning to the leader), the assertion strengthens from the row's "far fewer than 500" to **exactly 1**, the suite drops from ~0.09 s to 0.00 s, and both mutants are now caught outright (0 survivors, 0 timeouts across the staged diff's 40 mutants). Fixing the harness also exposed a real defect in `Join::poll`, which pushed a fresh waker clone on *every* poll — unbounded growth for as long as a leader's fetch runs, since an executor may legally poll a pending future any number of times; now guarded by `Waker::will_wake` | done |
| M1.21 | Conformance suite completion: run against MinIO, record the backend matrix | Serves FR-31. `scripts/gates/m1-complete.sh` runs the suite against the fake, starts a pinned MinIO container on a kernel-assigned loopback port, creates the bucket, exports the credentials, runs the suite **twice against the same bucket**, and delegates to `scripts/check-conformance-matrix.sh`, which validates `baselines/conformance-matrix.txt`'s format and cross-checks it against the recorded roster in both directions. The format half also runs in pre-commit, reading the index; the agreement half takes `--against-roster` and runs only from the milestone gate, because the `check-crate` hook writes a roster containing only `fake` and a partial roster is indistinguishable from the defect that check exists for. Six cases in `tests/gates/negative.sh` plant a reasonless row, a status typo, a duplicate row, a roster naming an unknown backend, a `verified` row nothing ran, and a missing matrix. ⚠️ **GCS took this row's own "reconsidering what against MinIO already only promised" branch.** ADR-0014 settled against live containers that no GCS emulator round-trips `object_store`'s request shape, and `M1.md`'s completion condition names only the fake and MinIO, so the matrix records `gcs` as `not-yet-run` with that reason beside `s3-real`/`gcs-real` (doc 10 #33). ⚠️ **Three defects found only by running things.** (1) `s3_minio.rs` claimed CI's T2 step set up MinIO; `.github/workflows/gates.yml` has no T2 job and never had one, ⚠️ **The first correction overshot, and review caught it from the git history.** "Never ran anywhere" is false: `M1.15` and `M1.16` both record running these tests against real MinIO by hand in their own commit messages, and `check-coverage.sh`'s exemption for this crate rests on that measurement. What was missing is anything that *re-runs* them — between one person's invocation and the next they were unenforced. Five copies of the CI claim corrected, including `testing.md`'s tier table; the CI job is recorded as `M1.34` rather than claimed. (2) The suite passed against MinIO once and failed on the second run: `conditional_write_if_absent` asserts a key is absent, creates it, and never cleaned up, so it was green only against a virgin bucket. (3) `record.rs`'s own test wrote sixteen invented names into the roster, so the artifact claimed eighteen tested backends of which sixteen did not exist — nothing noticed because nothing read the file back. ⚠️ **Six review rounds, and the durable lesson is that "verified by hand once" was the defect.** The comparison had no negative case because reaching it meant scaffolding a workspace *and* a daemon; splitting it into its own script made five of the six new cases cheap, and two silent regressions review had measured are now caught. Rounds 3 and 4 each blocked on bugs introduced by the previous round's fixes — a per-commit hook that failed every commit on a cleaned tree, an index read that broke `review.sh`'s own harness, an orphan sweep that deleted live processes' directories, and a `docker port` failure that killed the gate before its own guard could report it. All fixed and each verified by re-running the experiment that found it. The round-by-round narrative lives in the headers of `m1-complete.sh`, `check-conformance-matrix.sh` and `record.rs`, where it is next to the code it explains | done |
| M1.22 | madsim / `object_store` feasibility spike — not a code task | Serves no FR/NFR — a deferred spike shaping `standards/testing.md`, not an implementation of anything `requirements.md` lists (`roadmap.md`'s deferral table). A written finding: whether `object_store`'s (or `ADR-0008`'s chosen crate's) I/O can run under a deterministic simulator via `cfg`-swapped runtime, without changing crate structure (doc 10's resolved log). Recorded in `standards/testing.md` ("Deterministic simulation: the answer is a transport seam, not a runtime swap"), dated 2026-08-21 and reasoned rather than asserted. ⚠️ **The deferred question turned out to be the wrong one.** Under `madsim` specifically the answer is no: it needs `--cfg madsim` *and* a shim per I/O crate (its README lists five, for `tokio`/`tonic`/`etcd-client`/`rdkafka`/`aws-sdk-s3`), and `object_store` reaches the network through `reqwest` → `hyper` → `hyper-util` → `tokio::net`, for which no shim exists — so the swap would mean globally patching `tokio` beneath three third-party crates, with DNS, TLS and the wall clock in that path. That `madsim-aws-sdk-s3` exists is the tell: the ecosystem's own answer for S3 replaces the SDK, not the transport. ⚠️ **But `object_store` exposes a public request/response seam above the socket** — `HttpService`/`HttpConnector`, reached via `with_http_connector` on both the S3 and GCS builders — ⚠️ and the seam is **not** something this project's feature choice earned — `with_http_connector` carries no `cfg` beyond the module gate and `aws = ["aws-base", "reqwest", …]` implies `aws-base`, so the convenience features expose it identically; this workspace also enables `reqwest` and `ring`, so `object_store`'s "without `reqwest` or crypto" wording does not describe our configuration. A deterministic harness implements `HttpService` and answers from an in-process model: no `cfg`, no patched dependencies, no simulated TCP/TLS/DNS, crate structure untouched. ⚠️ **Verified by compiling and running throwaway probes**, then deleting them since this row is a finding rather than a code task; independent review re-derived the probe and drove a real `AmazonS3::get` through the model. Two wrinkles recorded so the next person does not rediscover them: `HttpService` is `#[async_trait]` (write the desugared boxed-future form and no dependency is needed), and `HttpResponse::builder()` is unreachable through the type alias, which *looks* like it forces a direct `http` dependency for the 404s and 412s any S3 model must return — review said it does, and testing the failing form showed `*resp.status_mut() = 404u16.try_into().expect(…)` infers `StatusCode` without naming the crate, so it does not. ⚠️ **Three review rounds were spent on the spike leaving live copies of the premise it had just refuted**, which is the same failure `M1.21` recorded. Round 1 found four — `roadmap.md`'s deferral row (which told readers to "take the resolved log"), ADR-0002, `M0.md`, doc 10 #32 — and round 2 found a fifth in `M10.md`, the milestone plan that *consumes* this finding, whose provisional task 1 read "simulation runtime selection wired behind the `cfg` seam": the seam the spike had just shown does not exist. ⚠️ **Fixing them one at a time is what let the fifth survive** — round 2 named doc 19 §DST and doc 05's recommendation ("`madsim` is the stronger fit") as well, and a tree-wide `grep` should have preceded the first fix rather than following the fifth finding. Round 3 then found a **sixth** in doc 13, which cites doc 05 §9 for madsim specifically — so the grep found sites, and review still found one after it. Every copy corrected in place and dated, the shape rows 200-203 of the roadmap table already use. Doc 10 #32 stays on the *open* list: only its mechanism is settled, not its cost, which it calls the largest single testing investment the project needs. The seam is above HTTP, so it does not replace T2: sockets, TLS, pooling and `object_store`'s own retry timing stay T2's job, and the tier table is unchanged | done |
| M1.23 | Fix `scripts/gates/m0-complete.sh`'s no-PyYAML fallback: strip quotes in the block-form branch the way the flow-sequence branch already does | Serves no FR/NFR — gate-correctness inherited from M0's boundary review, the same class as M0's own gate-tail rows. The block branch now ends `.strip("'\"")` exactly as the flow branch does, so all four spellings of a **`stages:` value** — bare, single-quoted and double-quoted block form, and flow form — parse identically with and without PyYAML. ⚠️ Scoped to `stages:` deliberately: the same missing strip survives on `entry:` and on the hook `id`, harmless today because their only consumer is a substring `grep`, and two further path divergences (block scalars `entry: >`/`entry: |`, and an explicitly empty `stages:`) are pre-existing and belong to no row yet. ⚠️ **Reproduced before fixing**: `- "pre-commit"` parsed as `'\"pre-commit\"'` on the fallback path and `'pre-commit'` under PyYAML. ⚠️ **A first attempt at this row claimed the inherited note had the direction backwards; review disproved that and it was wrong.** "False red" is right: with documents stating the true count the undercount is reported as a mismatch, and a dropped hook's `entry:` never reaches `PRECOMMIT_ENTRIES` either, so a genuinely invoked gate reads as "invoked by neither". The false *green* needs the documents to be independently stale by exactly the undercount. ⚠️ **The experiments that appeared to disprove review silently no-opped** — the reversion string did not match the file's bytes, and the extraction that fed them cut the heredoc at its opening `PYEOF` rather than its terminator, so "unfixed" was running the fixed parser. Same silent-substitution trap `M1.21` hit twice. The measurement that settled it: PyYAML 2, fixed fallback 2, unfixed fallback 1. `tests/gates/negative.sh` gains an inverted case (the same shape as the `THRESHOLD_RE` one): two genuinely pre-commit hooks, one flow and one quoted-block, against docs claiming one — only a gate that strips the quotes counts two and reports the mismatch. ⚠️ Proven load-bearing by reverting the fix, which makes the case "fail to fail" | done |
| M1.24 | Fix `scripts/check-drift.sh`'s header, which claims `_ms` matched `_message` | Serves no FR/NFR — same class as `M1.23`. Comment only, no behavioural change. ⚠️ **Checked rather than assumed**: `_message` contains no `_ms` at all (it reads `_me`…), so it never matched the unanchored form, and naming it as a false positive weakened the very point the sentence makes. `_msg` and `_msvc` do match unanchored and both stay. ⚠️ **Every other factual claim in that header was re-measured against the live `THRESHOLD_RE` while here, since adjacent claims have repeatedly also been wrong** — `err_msg` and `windows_msvc` are correctly *not matched* by the anchored pattern (the header uses "refused" for the gate rejecting a commit, so this row avoids the word), and `poll_msec`, `poll_ms`, `timeout_sec`, `timeout_secs` and `timeout_seconds` all still match, so the `msec`-spelling repair the header describes is intact. ⚠️ **That audit was scoped to `THRESHOLD_RE` match claims, and review found a header claim outside it that is false**: the sentence saying `m0-complete.sh` "makes it not depend on someone choosing the right word" describes a hand-maintained three-entry `NFR_CONSTANTS` map, which is exactly a list somebody must remember to extend. That is a live gate hole rather than a comment defect — `LIMIT=500` and `TIMINGS_KEEP_DAYS=30` are invisible to both — so it gets its own row (`M1.35`) rather than a wider edit here | done |
| M1.25 | Fix `tests/gates/negative.sh`'s `skip_case`: validate its third argument (not its fourth), and append to `SKIPPED_GATES` on the rejection path | Serves no FR/NFR — same class as `M1.23`. Both defects fixed. **A**: a bare number in the remedy slot — a count somebody put one argument early — was accepted as advice, printed to the reader as though it were a remedy, and left the count at its default of 1, undercounting with nothing failing; it is now refused, and recorded with the count it plainly meant. **B**: both rejection paths `return`ed without touching `SKIPPED_GATES`, so a refused call left the gate absent from `SKIPPED_CASES` — and since `m0-complete.sh` reads that line by *matching* names, an absent one is indistinguishable from a gate whose case ran; every exit now records through one `_skip_case_record` helper. ⚠️ **`run_case` cannot reach `skip_case`**, because it invokes a gate script against a scratch directory while `skip_case` is a function in the suite itself — which is why nothing had ever exercised it, and why both defects survived. The suite gains a small self-check section instead: eight checks in command-substitution subshells, so a deliberate `fail` never reddens the real run. ⚠️ **Eight checks, every one load-bearing against a distinct mutation** — measured by mutating each guard and each recording site in turn: dropping the numeric-remedy guard, the gate-name guard, or the fourth-argument guard each fails its own refusal check; making any of the three rejection paths stop recording fails its own `RECORDED` check; flipping any `return 0` to `return 1` fails that path's exit-status assertion; removing `10#` fails the zero-padded check; and recording `1` instead of `$cases` on the ordinary path fails the legitimate-skip check. ⚠️ **Every guard needed a *pair*** — a refusal check and a recording check — because neither half covers the other, and which half misses depends on the guard. A guard whose deletion makes the call *fall through* to the ordinary path (the gate-name one) emits an identical `RECORDED` line, so only the refusal half notices; a guard *weakened to silence* rather than deleted (the fourth-argument one) still records, so again only the refusal half notices — while the recording half is what catches a rejection path that stops recording. Review found both misses, one per round, each in a check written to satisfy rule 20a. ⚠️ **Four successive counts of this row were wrong before one was right** — *three* (a single combined revert that left one path intact), then *four* (review re-measured defect-by-defect and was right about that), then six once review found the refusal check passed with its own guard **deleted outright**: the call fell through to the ordinary path, which emits the identical `RECORDED` line, so the check pinned only that the name was recorded and not that it was refused. `testing.md` rule 20a's exact shape, in a check written to satisfy rule 20a. ⚠️ **Two bash traps found writing the harness, both of which made a check pass while the thing it tested was broken**: a bare `2>&1` after the last command in a substitution binds to *that command*, so `fail`'s stderr escaped the capture (the brace-group form captures it); and `out="$( ... )" || rc=$?` reads the status of the **last** command in the group — the `printf` — not of `skip_case`, so the guard written to catch a non-zero return stayed green through exactly that regression; the status is now captured beside the `eval` and printed as an `EXIT n` field the check greps. ⚠️ **Then the comment explaining that fix asserted something false about bash, and so did its replacement, in opposite directions.** The measurements, since the conclusion is what keeps going wrong: `x=$(false)` *does* abort under `set -e` (the assignment takes the substitution's status), `x=$(false; echo reached)` *does not* (the group's status is the last command's, and errexit is not inherited by commands inside a substitution unless `inherit_errexit` is set). Two different claims; each comment asserted one while meaning the other. The replacement also sent readers to `lib.sh` for a `shopt -s inherit_errexit` that lives at the top of `negative.sh` itself — where finding nothing invites deleting the real line, on which all 59 `run_case` invocations depend (55 is only the column-0 subset; the other four sit inside `if _have_*` branches and depend on it identically) — ⚠️ a number `M1.25` shipped wrong in this row while its own file comment carried the correction, recorded as that row's major finding and paid off by `M1.26`. Two further review findings fixed: the gate-name check now runs **first**, so a call wrong in two ways is diagnosed as the typo it is in one round rather than two; and `_skip_case_record` uses `10#`, since a count written `08` reached `$(( ))` as an invalid octal literal and died under `set -e` before `SKIPPED_COUNT` printed — the precise misdiagnosis the function's own `return 0, deliberately` note exists to prevent | done |
| M1.26 | Harden `gates.yml`'s `SKIPPED_COUNT` step: the `if [ -z "$n" ]` guard is unreachable under `pipefail` (a non-matching grep kills the step before the guard runs), and the step has no `if: always()` | Serves no FR/NFR — same class as `M1.23`. `\|\| true` on the assignment restores the guard's reachability; `if: always()` added so an earlier failing gate in the same job does not skip the check that would have caught a second commit's defect. ⚠️ **Both reproduced before fixing.** Under the step's own `set -euo pipefail`, a non-matching `grep` exits 1 and `set -e` kills the step at the assignment, so the `if [ -z "$n" ]` guard written for exactly that case never ran — an absent `SKIPPED_COUNT` line surfaced as a bare shell failure rather than the diagnostic. `|| true` restores it. ⚠️ **Not verified by pushing**: this row proposed a scratch-branch push, and pushing is not something to do unasked. The step's body was run verbatim against three fixtures instead — a clean run (exit 0), a file with no `SKIPPED_COUNT` line (the guard now fires with its own message), and `SKIPPED_COUNT 2` (reports the unproven gate and prints `SKIPPED_CASES`). `if: always()` is the half that cannot be verified locally at all, since it is GitHub's scheduler rather than the script that honours it; it is a one-line declarative change whose semantics are documented, and the honest statement is that this commit did not observe it | done |
| M1.27 | Correct the nine dependency pins `M0.30` added: the justifying comment is wrong for eight of them, and the clippy pin embeds `workspace` as a label only because that fixture happened to pass no crate argument | Serves no FR/NFR — same class as `M1.23`. ⚠️ **The row's own count was right, and a first attempt at this task wrongly "corrected" it.** `M0.30` added exactly nine pins — `build-index.sh --check`, `check-requirements-trace.sh`, three `check-hot-path-bench.sh` and four `check-crate.sh` — confirmed by diffing the pinned `run_case` lines across `81b2d99`. Measured over those nine, only `check-hot-path-bench.sh`'s first fixture emits more than one *kind* of failure; the other eight emit exactly one. So the comment's stated reason was wrong for eight of nine, as written. ⚠️ The first attempt measured the cases *nearest the comment* instead — seven of which are portability pins that pre-date `M0.30` and are owned by a different comment — and concluded three-of-ten, which is arithmetically right about the wrong population. Review found it by going to `git show`, which is where the question was answerable all along. The comment now names the nine it governs and states what rule 20a actually says — the test is the fixture's output, not the gate's branch count, and "a gate with nine `fail` branches whose fixture trips exactly one of them needs no pin". ⚠️ **So eight of these nine carry a pin the rule does not require**: harmless extra specificity and cheap insurance for the day one of those gates grows a property, but not an obligation, and a second attempt at this row cited rule 20a as *requiring* them — false in a new direction, caught by review reading the standard. ⚠️ The clippy fixture now passes `k` explicitly and the pin reads `clippy (k): warnings denied` — `workspace` was the label only because no argument was passed, and the crate-argument form makes it true by construction, keeps `clippy` in the pin so a future `rustdoc (...): warnings denied` cannot satisfy the case, and gives `check-crate.sh`'s per-crate scope its only case in the suite. Load-bearing: disabling the clippy branch makes the case report "ok on a broken artifact". | done |
| M1.28 | Correct `bin/oqueue/README.md`'s claim, falsified by M1's first commit that writes a connection loop: "every library crate in this workspace is written against traits rather than against S3, a socket or a clock" | Serves no FR/NFR — same class as `M1.23`. ⚠️ **The row named the wrong falsifier, and it has not happened.** No commit has written a connection loop: `oqueue-broker/src/lib.rs` is ten lines and contains no socket. What actually falsifies the sentence is `oqueue-store`, since `M1.15` gave it a real S3 backend — it is a library crate, it names `object_store` throughout, and `check-sans-io.sh` exempts it from the object-storage pattern **only**, holding it to the socket and clock patterns like every other library crate. So the claim has been false since `M1.15` for a reason the row did not name, and would have been falsified a second time later by the loop. ⚠️ **A first replacement made three new false claims and review caught all three**, which is the same defect class this row exists to close. It said `check-sans-io.sh` exempts the two crates "for those patterns and nothing else" — in fact the store scan is nested *inside* the broker guard, so `oqueue-broker` is exempt from all three, and review proved it by adding `object_store::` to that crate and watching the gate stay green. It said "exactly one crate implements each seam" — `ObjectStore` is implemented in `oqueue-core` (the fake plus three production decorators) and `oqueue-store`; `KeyProvider` in `oqueue-core` and `oqueue-crypto`. And it placed "the real clock in `oqueue-broker`'s I/O shell", which asserts code that does not exist **and decides `M1.29`** — the still-`todo` row one bullet below, which owns exactly that contested question. What ships instead claims only what is enforced today: a rule with named exceptions, the two exemptions stated as the script actually implements them, and no prediction about where anything will live. The Invariants table's "only place a concrete implementation is named" row is narrowed to the composition choice, since `oqueue-store` names `S3Store` because it defines it | done |
| M1.29 | Correct the `Clock` cell that frames `architecture.md` and ADR-0004 as opposing sides of a disagreement neither states | Serves no FR/NFR — same class as `M1.23`. The cell lives in `crates/oqueue-broker/README.md`, not in `architecture.md` or ADR-0004. ⚠️ **There was no disagreement to settle**: read side by side, the three documents answer different questions. `check-sans-io.sh`'s clock exemption says where the real implementation *lives* ("in `oqueue-broker` per the same reasoning as the socket exemption"); `architecture.md:59` says where concrete types are *chosen* (`bin/oqueue`, the composition root); and ADR-0004 takes no position on where the *real* one lives (it does place the **fake**, beside the trait in `oqueue-core`, rejecting `oqueue-testkit`) — its one mention of "`M1`'s real `Clock`" is about NTP clamping having no gate. ⚠️ **Those are the same two answers `ObjectStore` already has**, and nobody calls that contested: `S3Store`'s code lives in `oqueue-store` while `bin/oqueue` is where it *would* be picked — ⚠️ *would*, because that wiring is not written: `bin/oqueue` has no `oqueue-store` dependency today, which is `M1.39`'s question. So the cell now says the row does not claim `Clock` because `check-sans-io.sh` says the real one lives here, which makes "names no concrete `Clock`" a negative invariant this crate is expected to break — the socket defect `M0.31` fixed. ⚠️ Not "because the implementation is not written yet", which a first correction said and review rejected: unwritten is the condition under which the claim would be *true* today, so it argues the opposite of its conclusion. ⚠️ **This unblocks a question three earlier rows deferred to**: `M1.28`'s first replacement was blocked partly for deciding it by implication, and the broker README promised "whichever way it goes, the loser is a document to correct in the same commit" — there is no loser in the sense that promise meant — but ⚠️ **a document did state the disagreement as fact and review caught it left behind**: `crates/oqueue-broker/AGENTS.md`'s rule 2 said "`README.md`'s Invariants row records the disagreement; M1 settles it and corrects whichever document loses", which this row makes false. `M0.31` wrote **three** copies of the framing — the broker README, its `AGENTS.md`, and `M0.31`'s own row in this file — and a first version of this task corrected one, then a second corrected two. All three now carry the same reading. ⚠️ **And a first version of this row committed the very conflation it dissolves**, twice: it inferred from "the implementation lives here" that the clock "is not something `bin/oqueue` hands this crate" — a wiring decision `check-sans-io.sh` does not make, contradicted by `main.rs` and by this row's own `ObjectStore` analogy — and it asserted that `bin/oqueue` "picks" `S3Store` when `bin/oqueue` has no `oqueue-store` dependency at all. Both corrected: where a thing lives is not who constructs it, and the analogy now says `bin/oqueue` is where it *would* be picked, marking the wiring as unwritten | done |
| M1.30 | Fix `check-budget.sh`'s NFR-56 blind spot: a run containing one gate over `COMPILING_GATE_MS` currently has the whole suite judged against the *remainder*, silently granting the other gates the full 10 s budget instead of what is actually left | Serves NFR-56. ⚠️ **Took the row's second branch — the rule is deliberate and already documented; what it lacked was a test.** `check-budget.sh`'s header states the rule ("take out every gate over `COMPILING_GATE_MS`; if what remains is under budget, the run was a build"), names two weaker rules tried and rejected *by measurement*, and states the residue rather than hiding it: two gates at 5.1 s each read as two small builds, "the price of a single-run heuristic". Changing that to compare the whole suite's wall time — the row's first branch — would fail every cold clone and every `cargo clean`, which is the measurement (28.6 s, almost all of it `check-crate.sh` compiling) the rule exists because of. ⚠️ **The real hole was that nothing pinned the exemption's boundary**: dropping the `&& warm_ms <= BUDGET_MS` conjunct, so *any* compiling gate exempts the whole run, left the negative suite green — and that is precisely the rule the header calls rejected because it "would have made this gate unfailable from `M0.17` onward, since a mutation run is minutes". A case now plants the header's own worked example (21 600 ms of build plus 19 904 ms of eroded suite, every warm entry under the threshold so none is mistaken for a second build) and requires the gate to fail. Load-bearing: with the conjunct dropped it reports "ok on a broken artifact". ⚠️ **One direction only, and the residue is stated rather than hidden**, as that file's header already does for its other one: the case pins the *conjunct*, not the definition of `warm_ms`. Review mutation-tested it — `warm_ms=$((total_ms))`, `warm_ms=$((total_ms - slowest_ms))` (the slowest-only rule the header says it rejected) and `<` for `<=` all survive the whole suite. The exemption's positive half cannot be a case at all, since a case passes only when the gate fails. ⚠️ **And `M1.md`'s bullet, which is sharper than this row's text, is confirmed by the repo's own trend file**: 14 of 123 recorded runs were exempted, *every one* of them `compiling:check-mutants.sh` — exactly the gate the bullet predicted would trip the threshold on ordinary commits — with the worst exempted run at 70 179 ms. The script's header already named the missing piece ("a trend that is permanently `compiling` is itself the signal — and nothing reads that yet"), so the exemption branch now reads its own `suite.tsv` back and says so when the exemption rate crosses a quarter of recorded runs — naming precisely what is unbounded (the compiling gates' own time) rather than claiming the run went unmeasured, which it did not. Both thresholds are hoisted to named constants beside `BUDGET_MS`, since every other number in that file is. ⚠️ Today's 11% does **not** trip it, and the comment says that rather than warning about the state it was written in. ⚠️ **Still a `skip`, not a `fail`** — escalating would block a commit on a cold clone, which is the case the exemption exists for, and that is a policy change rather than a gate fix: `M1.40` | done |
| M1.31 | Route `security.md` to the code that actually holds secrets: its `applies_to` excludes `oqueue-core` (which holds `Redacted`, `WrappedKey`, `KeyId`, `Error::SecretRejected`, `FakeKeyProvider`) and `bin/oqueue` (which holds the `KeyProvider` choice) | Serves no FR/NFR — same class as `M1.23`. ⚠️ **Verified before fixing**: `which-standards.sh crates/oqueue-core/src/key.rs` — the file defining `WrappedKey`, `KeyId` and `FakeKeyProvider` — selected seven standards and `security.md` was not among them. `applies_to` now names `*oqueue-core/*` and `*bin/oqueue/*`, matching that field's crate-glob convention for code (its seven code entries are all crate globs; the other two, `*.pem` and `*.key`, are file patterns for key material rather than a file list of sources) rather than naming `key.rs`/`redacted.rs`/`error.rs`, which would go stale the first time key material moved. ⚠️ The cost is noise: every `oqueue-core` diff now hands the reviewer `security.md`, an eighth standard where there were seven. That is the trade the convention already makes for the seven crates it already named. ⚠️ **`which-standards.sh` had no test of any kind**, and `run_case` structurally cannot give it one — it is not a pass/fail gate, it prints a list, and this suite's pass condition is a non-zero exit. Pinned the way `M1.25` pinned `skip_case` instead, with the reason stated per `testing.md` rule 20a: three checks assert that a key-material diff and a composition-root diff each select `security.md`, and that a docs-only diff does not — the third so the patterns cannot pass by matching everything. Both positive checks are load-bearing: reverting the two `applies_to` entries fails exactly them. ⚠️ **And a first version of those checks had a hole review measured**: they merged stderr into the captured output and discarded the exit status, and `which-standards.sh` reports a missing `applies_to` as `PROBLEM docs/internal/standards/security.md …` on stderr — a line containing the very path they grep for. So deleting `applies_to` outright made all three print `ok` while `security.md` routed to nothing. The streams are separated and the exit status checked now, and deleting the field fails all three | done |
| M1.32 | Consolidate the triple-duplicated hand-rolled TOML parser | Serves no FR/NFR — same class as `M1.23`. ⚠️ **All three divergences reproduced before fixing, and the damage was worse than the row said.** ⚠️ **The three copies never drifted from *each other*** — they were behaviourally identical on every input this repository has, one `read_text(encoding=…)` apart, so `check-readmes.sh`'s "flag it in review if the two ever drift" note had nothing to fire on. ⚠️ Review found the one input that distinguishes them — under `python3 -X utf8=0` with `LC_ALL=C`, the un-encoded copy raises `UnicodeDecodeError` on a manifest whose comments carry `⚠️`, which every manifest here does — unreachable on today's runners but not on the 3.9/3.10 ones `manifest.py`'s header contemplates. What they had drifted from is another reader in the same file, `check-layering.sh`'s `sections()`, already fixed for the header defects all three still had. ⚠️ **Which is why the first consolidation reintroduced two defects that `sections()` was already fixed for** — an anchored header pattern, and returning nothing for a `[`-line it cannot name, which leaves the previous table sticky. That docstring records both as measured bugs ten lines from where the new import went in, so the consolidation had picked the *weaker* of the two readers. `manifest.py` now adopts `sections()`'s reading. On one manifest carrying every named spelling, `runtime_deps` returned `{harness, libc, name, tokio}` — reading a `[[bench]]` table's `name` and `harness` keys as runtime dependencies of the crate — and `package_name` returned `None` for `name = 'oqueue-broker'`, which `check-layering.sh` reports as a manifest with no `[package]` name. The cause is one shared pattern, `^\[([A-Za-z0-9_.\-]+)\]$`, that matches neither `[[bench]]` nor `[target."cfg(unix)".dependencies]` — and ⚠️ **an unmatched header did not end the previous section**, which is what turned incompleteness into wrong answers. Single-quoted values and quoted dependency keys (`M1.23`'s review) were invisible for the same reason. One reader now: `scripts/lib/manifest.py`, imported by `check-layering.sh` and `check-readmes.sh`, whose own copy carried the docstring "Mirrors check-layering.sh's parser — see this file's header for why it is not shared", a comment asking to drift. Target-scoped dependencies still count, but now deliberately rather than by the accident of a failed match. ⚠️ **Eleven checks pin it** — `run_case` cannot, since the module returns values rather than failing — one spelling each. ⚠️ **A single manifest carrying every case cannot isolate them**, which two rounds of review measured: whichever header ends the dependency table first hides the ones after it, so a mega-fixture left three of the reader's four fail-safe branches unpinned and its own comment described a defect it did not exercise. Split one-per-check, every branch is caught by exactly its own: array-of-tables, unparseable header, trailing comment on a *non*-dependency header, trailing comment on `[dependencies]` itself (the direction the fail-safe fallback cannot cover for, since it hides every dependency rather than adding one), target-scoped inclusion, and single-quoted values. ⚠️ **Two successive splits still left branches unpinned whose inputs are ordinary Cargo, and review measured every one.** Round three: dropping the named-sub-table branch makes `[dependencies.oqueue-core]` invisible while recording `path` and `version` as dependency names, and relaxing the target test to `segments[0] == "target"` alone crashes on `[target...dev-dependencies]`. Rounds four and five: dropping `_KEY`'s single-quote alternative loses a `'oqueue-core' = { … }` dependency outright, and turning `_SEGMENT`'s skip-a-character fallback into a `break` loses every dependency under a cfg whose text embeds an escaped quote; dropping `_KEY`'s dotted-suffix alternative makes **every `foo.workspace = true` dependency invisible** — the form nearly every manifest here uses — with the whole suite and `check-layering.sh` still green; an off-by-one in the target segment index records a dependency literally named `dependencies`; and without the `[package]` section guard the first `name =` anywhere wins, so a benchmark's name becomes the package's. ⚠️ **Six branches remain unpinned and are named rather than claimed away**, from review's 57-mutant matrix: admitting `build-dependencies` into the dependency test; relaxing `package_name`'s key test so a `[package]` whose `version` precedes its `name` returns the version; `find`→`rfind` on the closing bracket; `_key_of` returning empty; dropping `.strip()`; and a prefix match for the `[package]` section. None is reachable by any manifest in this tree — none has `[build-dependencies]`, a target table or an array-of-tables, and all thirteen put `name` first — which is why they are recorded rather than fixed here. ⚠️ `is_array` is unpinnable rather than unpinned: it changes an answer only for `[[dependencies]]`, which Cargo rejects. ⚠️ **That every round found more is itself the finding**: a shared reader concentrates the blast radius, so "the tests pass" says less about it than about any one of the copies it replaced. ⚠️ Found doing it: `copy_gate` copied a gate without what it imports, so six unrelated cases failed "not for the reason the fixture plants"; it now brings `scripts/lib/` too, and the gates resolve that directory from their own location rather than the cwd | done |
| M1.33 | `RateLimitPolicy::Ramping` collapses GCS's per-op-class starting rates into one `initial_per_sec`, on a doc-04 citation the source does not support — found by M1's checkpoint review | Serves FR-31. `docs/researches/04-object-storage-s3-gcs.md` §3 ("Throughput & Request-Rate Limits") states GCS buckets start at roughly **1,000 object-write requests/s** and **5,000 object-read requests/s** — a 5x split — not the single undifferentiated rate `rate_governor.rs`'s doc comment on `RateLimitPolicy::Ramping` cites doc 04 §3 for ("does not split GCS's ramp by read/write"); the same claim is repeated in `backlog.md`'s own (done) `M1.13` row. `RateGovernor::new` currently seeds both `write_bucket` and `read_bucket` from the same `initial_per_sec` for a `Ramping` policy, so a governor built with either provider's documented starting rate is wrong for the other op class by 5x. Before `M1.17` wires a `RateGovernor` against real GCS traffic: either `Ramping` gains a per-op-class starting rate (mirroring `FixedCeiling`'s `writes_per_sec`/`reads_per_sec` split) with a test proving the two ceilings differ at `t=0`, or the doc comment and `M1.13`'s row are corrected to state — with the real citation — why one shared value was deliberately chosen instead | done |
| M1.34 | CI has no T2 job, so every containerized test runs only where a human runs it | Serves FR-31 — found by `M1.21`, which went looking for the T2 step `s3_minio.rs` described and found it absent from `.github/workflows/gates.yml` entirely. `scripts/gates/m1-complete.sh` now starts MinIO and runs the suite, but it is a milestone-boundary gate a person invokes, not something a push triggers: a commit that breaks the S3 backend is caught whenever someone next closes a milestone, which is not a schedule. A `gates.yml` job with a MinIO service container running `cargo test -p oqueue-store --test it -- --include-ignored`, and the same twice-against-one-bucket property the local gate asserts. ⚠️ Check what it costs first: `build.md`'s CI budget, and whether the container pull dominates | todo |
| M1.35 | `check-drift.sh`'s header claims `m0-complete.sh` closes the name-based-matcher hole; it closes it for three hardcoded names | Serves NFR-50 — found by `M1.24`'s review. `m0-complete.sh`'s `NFR_CONSTANTS` is a literal three-entry map (`COVERAGE_FLOOR`, `BUDGET_MS`, `COMPILING_GATE_MS`), and its visibility assertion runs inside a loop over exactly those, so a threshold whose name matches neither `THRESHOLD_RE` nor that map is unenforced for non-negotiable 2 while every gate reports `ok`. ⚠️ **Four are already live and already invisible**: `scripts/check-file-size.sh`'s `LIMIT=500`, `scripts/check-budget.sh`'s `TIMINGS_KEEP_DAYS=30`, and — added by `M1.30`, named so `check-drift.sh` sees them but still absent from the map — that file's `EXEMPT_RATE_THRESHOLD=4` and `EXEMPT_RUNS_FLOOR=20` — `LIMIT="${MAX_LINES:-500}"` would pass `check-drift.sh` green today. Correct the header sentence to its real scope, and add both constants to `NFR_CONSTANTS`; ⚠️ adding `LIMIT` fails the visibility half immediately, which forces a rename to something `THRESHOLD_RE` sees (`FILE_LINE_LIMIT`), exactly as `M0.15` renamed `MIN_CRATE_COVERAGE` — that is what makes this row testable rather than comment-only. Also in scope, same header: two self-quotations that point at text the file no longer contains (`"cheap to dismiss on sight"`, retracted at its own line 65; and a `see "What this does not catch" below` naming a section that does not exist) | todo |
| M1.36 | `check-crate.sh`'s tests pin embeds `workspace` for the same incidental reason `M1.27` fixed on the clippy one | Serves no FR/NFR — found by `M1.27`'s review, recorded rather than fixed there because the row named only the clippy pin. `tests/gates/negative.sh`'s `check-crate.sh (failing test)` case pins `"tests (workspace): failing"`, and `check-crate.sh:234` builds that label from whatever scope it was given — `workspace` only because `invoke_crate_test` passes no crate argument. Reproduced. Same fix as `M1.27`: pass the crate explicitly so the label is true by construction. ⚠️ Note the failure mode is **loud**, not silent — a crate argument makes the case report "failed, but not for the reason the fixture plants" — so this is a latent-brittleness fix, not a live hole | todo |
| M1.37 | `oqueue-store` documents an in-memory backend it does not have | Serves no FR/NFR — found by `M1.28`'s review, same class as that row. `crates/oqueue-store/src/lib.rs:1` and `README.md:5` both say "`ObjectStore` implementations: S3, GCS, and an in-memory backend", and the README's Notes section explains at length how "the in-memory backend here is a real implementation, not a fake" and must pass the same conformance suite as S3. No such backend exists: `lib.rs` exports `GcsStore` and `S3Store` and nothing else. ⚠️ `check-readmes.sh` passes over it, because it checks a README's *sections* against the manifest rather than its claims against the code. Delete the claim, or say what it was meant to be — `M1.6`'s `FakeObjectStore` lives in `oqueue-core` by `contracts.md` rule 9, so the Notes paragraph may be describing a plan that rule already refused | todo |
| M1.38 | `bin/oqueue/src/main.rs` still states the universal `M1.28` removed from its README | Serves no FR/NFR — found by `M1.28`'s review, same class. The module doc reads "Every library crate is written against the trait seams in `oqueue-core`", which `oqueue-store` falsifies exactly as it falsified the README beside it. ⚠️ **This is how the claim spread in the first place**: `M0.31` removed it from the broker README and it survived in `bin/oqueue/README.md`, which `M1.28` has now corrected while `main.rs` — the crate's most-read file — kept it. Scoped out of `M1.28` because that row named the README only | todo |
| M1.39 | `bin/oqueue/src/main.rs`'s wiring comment is overtaken by `M1.15`/`M1.17` | Serves no FR/NFR — found by `M1.29`'s review, same class as `M1.28`. `main.rs` says "`Clock` and `ObjectStore` join it when a real implementation of either exists — `M1` writes the first", but `M1.15` and `M1.17` wrote `S3Store` and `GcsStore`, and `Wiring` still holds only a `Box<dyn KeyProvider>`. So the comment reads as though no `ObjectStore` implementation exists when two do. ⚠️ Decide which it is: the comment is stale and the wiring is simply not due until a caller needs it, or the wiring is genuinely owed and this is the row that adds it — `bin/oqueue/Cargo.toml` has no `oqueue-store` dependency either way | todo |
| M1.40 | Decide whether a persistently `compiling` suite should fail rather than skip | Serves NFR-56 — found by `M1.30`, which measured that 14 of 123 recorded runs took the compiling exemption, all of them `check-mutants.sh`, the worst at 70 179 ms. ⚠️ **Not "bypassed the budget entirely"** — a first version of this row said that and review showed it false: the branch is unreachable unless `warm_ms <= BUDGET_MS`, so on every one of those runs the non-compiling remainder *was* measured and passed. What is unbounded is the compiling gates' own time, and stating it the other way would argue for removing a check that is still working. `M1.30` made the rate visible; it did not change the verdict, because a `fail` there blocks a commit on a cold clone or after `cargo clean` — the case the exemption was built for. ⚠️ The question is a policy one and needs a decision, not a patch: does NFR-56 mean "the suite is under 10 s once warm" (today's rule, and the trend read-back is enough) or "under 10 s every run" (then the exemption is wrong and cold runs need somewhere else to go, e.g. a warm-up step or moving `check-mutants.sh` to CI, which `testing.md` rule 16 already half-proposes). Whichever, the loser is a document to correct — `requirements.md`'s NFR-56 row, or `check-budget.sh`'s header | todo |
| M1.41 | A change *to* a standard is reviewed without that standard | Serves no FR/NFR — found by `M1.31`'s review, which observed that its own packet judged an edit to `security.md` while `which-standards.sh` selected git, review and sdd for it and not `security.md` itself. ⚠️ **Not every standard has this shape, and a first version of this row said it did.** Measured across all fourteen: `git.md` and `review.md` self-select via `applies_to: ["*"]`, and `sdd.md` names `docs/internal/standards/*` — which is *why* an edit to `security.md` selects `sdd.md` at all. Three of fourteen already claim their own path, so the "design" answer is partly implemented in the field, and a standard can claim its own path today without touching `which-standards.sh`. Several standards also name no code: `sdd.md`, `build.md` (manifests) and `portability.md` (`scripts/`, `.agents/`). ⚠️ Decide whether that is a defect or the design — a standard's *content* is reviewed against `sdd.md` and `review.md`, which is arguably right, and self-selection would hand a reviewer the document they are already reading. If it is a defect, the cheapest fix is a pattern in each standard's own `applies_to`, as three already have, rather than a change to `which-standards.sh`; if not, say so where a reader will look, because it is not obvious | todo |

### Where this decomposition diverged from the plan

| Plan item | Task(s) | How |
| --- | --- | --- |
| 1. `ObjectStore` trait | M1.3 | merged with item 2 |
| 2. Core types | M1.3 | merged with item 1 |
| 3. Error taxonomy | M1.4 | blocked on M1.2's ADR, restated |
| 4. `Precondition` enum | M1.5 | — |
| 5. In-memory fake | M1.6 | — |
| 6. Fault injection | M1.8 | — |
| 7. Multipart/resumable types | M1.9 | — |
| 8. S3 backend | M1.15, M1.16 | split: core ops from multipart, mirroring `M0.21`'s pattern of splitting a row too large for one commit |
| 9. GCS backend | M1.17 | — |
| 10. `copy_range` | *(none — dissolved)* | `M1.18` found `object_store` exposes server-side range copy for neither S3 nor GCS; moved to `M5.md` task 7, see `M1.18`'s own row and ADR-0015 |
| 11. Retry policy | M1.12 | — |
| 12. Rate governor | M1.13 | — |
| 13. Key layout | M1.11 | — |
| 14. Ranged-GET | M1.19 | — |
| 15. Chunk addressing + single-flight | M1.20 | — |
| 16. Op-class accounting | M1.14 | — |
| 17. Conformance harness | M1.10, M1.21 | split: harness-against-the-fake from the MinIO/GCS run that is the completion condition's real content |
| 18. Region header `alg` | *(none — dissolved)* | `M1.7` found no region header exists in `M1` to add a field to; moved to `M3.19`, see `M1.7`'s own row |
| — | M1.0 | no plan item: opening the milestone |
| — | M1.1 | no plan item: ADR-0008, from "Decisions required first" |
| — | M1.2 | no plan item: ADR-0009, from "Decisions required first" |
| — | M1.22 | plan names it as non-code, carried as a row anyway so it is tracked |
| — | M1.23–M1.32 | no plan item: `milestones/M1.md`'s inherited-findings section, not its provisional-task list |
| — | M1.33–M1.41 | no plan item: found by review after the decomposition was written — `M1.33` by the checkpoint review, `M1.34` by `M1.21`, `M1.35` by `M1.24`, `M1.36` by `M1.27`, `M1.37` and `M1.38` by `M1.28`, `M1.39` by `M1.29`, `M1.40` by `M1.30`, `M1.41` by `M1.31` |


## M0: Workspace, contracts, and quality gates

The Cargo workspace, the eleven crates, every `oqueue-core` trait seam with a
fake beside it, and the two gates whose constants needed a workspace before they
could be chosen. No broker behaviour: what M0 delivers is the shape everything
else is written into, plus the first real exercise of gates M-1 wrote against an
imagined codebase.

⚠️ **M0 carries 33 rows, over `sdd.md`'s cap of 20, and here is the argument the
standard requires.** ⚠️ **The count went stale once, at 30**, surviving two
commits that added rows — and the first attempt at this very sentence wrote 32
against a staged tree of 33, because the row adding the correction is itself a
row. `roadmap.md` met the same problem and deleted its copy; this one keeps a
number only because `sdd.md` asks the argument to say what it is arguing about.
Treat it as a claim to re-derive rather than to trust:
`grep -cE '^\| M0\.[0-9]+ \|' docs/internal/product/backlog.md`. The cap's own rationale is "two milestones wearing one
name". M0 is named *Workspace, contracts, and quality gates*: `M0.0`-`M0.13`
build the workspace and the contracts, `M0.14`-`M0.18` and `M0.21` build the
quality gates, and the third of those is a thing the milestone is named for
rather than new scope smuggled in. The rows past 20 are not scope either —
`M0.20` and `M0.22` act on M0's two reviews, and `M0.21` is work split *out* of
`M0.8` because that row had grown too large for one commit.

⚠️ **The last four rows are a third bucket, and the argument for them is
different.** `M0.23`-`M0.26` come from M0's boundary review and none of them is
new capability: `M0.23` and `M0.26` are gates M0 already claimed and that do not
hold, and `M0.24` and `M0.25` are sentences **M0 itself falsified** — `M0.24`'s
two files are the ones `M0.1` was written to correct. So they are not a second
milestone wearing this name; they are this milestone's own claims not yet being
true. ⚠️ **That is a reason and not a licence** — `sdd.md` warns the cap is not
permission to grow a milestone rather than finish it, and the honest form of
that warning here is that a fifth bucket would be one. Anything M0's review
found that is *new* work went to the milestone that receives it, which is why
five of its nine majors are arguments rather than rows.

⚠️ **`M0.27`-`M0.29` are the fifth bucket the paragraph above warned about, and
the warning stands.** They are the minors M0 wrote into commit bodies and never
scheduled — so they are not new capability either, and by the third bucket's own
test they belong here: `M0.27` and `M0.28` are gates M0 claimed that do not hold
in a case nobody watched, and `M0.29` is sentences M0 falsified. ⚠️ But this is
the point at which "M0's own claims not yet true" stops being a reason and
starts being a way to keep a milestone open, because the *supply* of recorded
minors is renewed by every commit that fixes one. **The bound is that these
three rows are scoped to what was written down before they opened**, not to
whatever the reviews of these three rows produce; anything they record goes to
M1 or to the procedure decision `roadmap.md` defers into M2.

⚠️ **A sixth bucket arrived anyway, and here is why it is not the growth
`sdd.md` names.** `M0.30`-`M0.32` come from M0's **closing** boundary review —
the one that cannot happen until the last row is done, so its findings cannot
be scheduled before the milestone is otherwise finished. `M0.30` is the largest
and the closest to new capability: a CI step and a standard rule. It is here
rather than in M1 because what it fixes is `M0`'s own gates ceasing to run *at
the moment M0 closes* — a defect with a deadline of this commit. `M0.32` is sentences
`M0.30` falsified, which is the third bucket's test applied to the third
bucket. ⚠️ `M0.31` is **not** that — its two documents were false before M0's
tail touched them — and it is here because the closing review raised them and
`review.md` rule 16 needs a major finding to name a row. That is the narrow
licence: a finding *this milestone's own review* made, not any pre-existing
false sentence anyone notices.

⚠️ **And that is the last one.** The rule this milestone has now demonstrated
five times is that a closing review's findings regenerate the set they are
bounded by. `M1` inherits what M0's closing review found and did not fix —
`M1.md` carries it — and a **seventh** bucket would be the growth without
qualification, with nothing left to distinguish it from simply not ending. ⚠️ The forcing
issue: a milestone at exactly the cap has **no legal way to act on its own
boundary review**, since `review.md` rule 16 needs a major finding to name a
row. A decomposition rule that forbids the outer loop from working is the rule
being wrong, and `sdd.md` now says so.

Plan: [milestones/M0.md](milestones/M0.md). ⚠️ **The plan was an input, not this
list.** Eighteen plan items became nineteen tasks, and the mapping is not
one-to-one: six items merged into three, one dissolved into two others, and five
tasks have no plan item at all. ⚠️ `M0.19` is a twentieth, added mid-milestone
from what the loop itself turned up; it is outside that mapping, which describes
the decomposition rather than the milestone's final shape. "Where this decomposition diverged from the
plan" below carries the item-by-item mapping, because `sdd.md` §Decomposition
says the divergence is evidence about how far ahead this project can usefully
see — and a count is not that evidence if it is only asserted.

| ID | Task | Acceptance | State |
| --- | --- | --- | --- |
| M0.0 | Open M0: decompose it here, correct `milestones/M0.md`'s `**Serves:**` line and `roadmap.md`'s requirement-coverage row to name every requirement M0's tasks actually cite, flip the State cells in `roadmap.md`, `milestones/M-1.md` and `README.md`, and record where this decomposition diverges from the plan | `check-requirements-trace.sh` passes with M0's plan and the roadmap row agreeing; every **other** M0 row names at least one FR/NFR that `requirements.md` lists; M-1 reads `complete` and M0 `in progress` in all three files that state it; `build-index.sh --check` and `check-portability.sh` pass. ⚠️ Serves no FR/NFR itself and says so — it is `sdd.md`'s milestone-opening step, and a process task claiming a requirement would be worse than one admitting it has none | done |
| M0.1 | Correct the two agent-facing files that still say M-1's gates do not exist — `AGENTS.md` and `.agents/skills/README.md` | ⚠️ "M-1.7 through M-1.12" names six **tasks**, not six scripts: five landed and one did not. Every script those five produced exists and passes — `check-drift.sh`, `check-tests-kept.sh` (M-1.7), `check-layering.sh`, `check-sans-io.sh` (M-1.8), `check-reviewed.sh` (M-1.9), `check-core-contract.sh` (M-1.10), `check-unsafe.sh` (M-1.11) — and only M-1.12's `check-budget.sh` does not, which `M0.16` writes. Both files currently tell a reader that `check-drift.sh`, `check-tests-kept.sh`, `check-layering.sh`, `check-sans-io.sh`, `check-core-contract.sh` and `check-unsafe.sh` are unenforced preferences — the exact gates M0.2's acceptance requires to run and pass, so an executor reading `AGENTS.md` first is told the milestone's first real task is unverifiable (NFR-50). ⚠️ `README.md` and `CONTRIBUTING.md` said the same and were corrected in M0.0's own commit rather than left to this row: M0.0 flipped the roadmap table eighteen lines above README's Contributing paragraph, and the two public-facing files disagreeing with each other — and with the table directly above one of them — is not a state to ship for the length of a task, however short. ⚠️ Correcting the *reason* is not the same as opening the project to code — whether contributions are accepted is a decision, not a consequence of M-1 finishing, so state the true reason and leave the policy alone unless someone changes it deliberately. Acceptance: neither file claims a script is missing that is present, or that M-1 is still in progress; neither enumerates which scripts are still missing, both say where that is recorded, and a reader following the pointer reaches rows that are true on the day they read them. ⚠️ **The correction must not replace one hand-maintained list with another.** `AGENTS.md` is loaded into every session, five of the scripts it would name land during M0 itself, and a list written here goes stale at M0.2 and stays stale for sixteen tasks — which is precisely the failure `M-1.47`, `M-1.49` and `M-1.52` chased three times before the fix was to delete the list and point at `backlog.md`. Both files say instead that M-1's gates exist and run, that a rule whose script is still missing is still a preference, and that **`backlog.md` is where to look for which** — it is authoritative, every gate reads it, and it is updated by the task that closes each row. ⚠️ `security.md` rule 5's `scripts/fuzz.sh` and rules 6–7's `check-secrets.sh` are absent too and are **scheduled nowhere**, in M0 or after it — pointing at `backlog.md` covers them honestly where an enumeration of what M0 happens to build would not, and **their being unscheduled is a finding for M0's boundary review**, recorded here so the review need not rediscover it. A file that says "these gates are missing" and then lists the wrong set is no more use than one that says nothing; `check-portability.sh` and `build-index.sh --check` pass. ⚠️ First because `AGENTS.md` is loaded into every session before anything else is read | done |
| M0.2 | Workspace root manifest, the `oqueue-core` skeleton, and `scripts/check-crate.sh` — the smallest tree that actually compiles **and is checked** | `cargo check --workspace --all-targets` exits 0 for both `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` (NFR-40); the root manifest pins the resolver, `[workspace.package]`, `[workspace.dependencies]` and `[workspace.lints]`; `oqueue-core` carries `#![forbid(unsafe_code)]` and no workspace dependency (NFR-52, NFR-53) with a `README.md` and `AGENTS.md`; `check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh` and `check-readmes.sh` each run against real Rust for the first time and pass. ⚠️ **`scripts/check-crate.sh <crate>` is in this commit too**, and this is not scope creep: the `tdd` skill's definition of done and the `milestone` skill's own loop both name it, doc 19 §5 specifies it (fmt and clippy scoped to one crate for in-session speed, workspace-wide in CI), and it exists nowhere — so without it this commit introduces the first Rust in the repository and nothing in `.pre-commit-config.yaml` runs a single cargo command against it. A commit that adds code no gate checks is the shape `NFR-50` exists to prevent. ⚠️ **`README.md`'s status banner — "There are zero lines of Rust in this repository" — is corrected in this same commit**, by the row that makes it false rather than by a later one; the project's most-read sentence being wrong for sixteen tasks is not a thing to schedule. Acceptance for that half: `check-crate.sh` runs `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test`, scoped to a named crate and workspace-wide with no argument; it is wired into `.pre-commit-config.yaml` and the push-triggered CI; and `tests/gates/negative.sh` gains a case proving it fails on unformatted code, on a clippy warning, and on a failing test — M-1.15's standard, which is why this is one commit and not two | done |
| M0.3 | The build profiles, and **ADR-0001** recording them | The profiles doc 18 **§3.6** names are present in the **root** manifest — member-crate `[profile]` sections are ignored — with `release` at `lto = "thin"` and `codegen-units = 16`, a separate `dist` at fat LTO and `codegen-units = 1`, and `bench` inheriting `dist` and differing from it in **exactly one way**, `debug = true`. ⚠️ Two traps doc 18 §3.2 names, both silent: `lto = false` with `codegen-units = 1` performs **no LTO at all** — strictly worse than the stock profile — and `cargo bench` inherits `release` unless pinned, so out of the box you benchmark codegen you never ship. ADR-0001 records why thin LTO is mandatory rather than optional in a multi-crate workspace (doc 18 §3.1: without it a non-generic `pub fn` does not inline across an rlib boundary) and why fat LTO is **not** in `release` but is in `dist`: doc 18 §3.3 measures fat LTO with `codegen-units = 1` at **~2.3× build time**, paid by every developer and CI release build. ⚠️ That cost is *not* NFR-56's — the pre-commit suite builds dev and test profiles and never touches `release`, so an ADR arguing fat LTO out of `release` "against the pre-commit budget" would be recording a reason that is not true. `cargo check --workspace` exits 0 under every profile on **both** targets (NFR-40), and `cargo build --profile bench --workspace` exits 0 on the host — ⚠️ `build`, not `check`, only on the host, because aarch64 has no cross-linker until M13 and a criterion that outruns that would have to be weakened the moment M0.12 adds a binary | done |
| M0.4 | **ADR-0002**: the async runtime, and how `oqueue-core` expresses an async seam | The ADR chooses the runtime; states that `oqueue-core` names it nowhere (NFR-51); decides native `async fn` in trait versus `#[async_trait]` for `ObjectStore` and `KeyProvider` **with the `dyn`-compatibility consequence stated**, since that decides whether the broker's I/O shell can hold a trait object at all; cites doc 05 §3 and doc 10 #32, including that doc 10's resolved log supersedes #32's "touches every crate that does async" with madsim swapping the runtime by `cfg` (⚠️ refuted by `M1.22`, 2026-08-21 — see ADR-0002's own dated note and `standards/testing.md`'s "Deterministic simulation"). No code. Blocks M0.9, M0.10, M0.11 | done |
| M0.5 | Core types and IDs: `TopicId`, `PartitionId`, `Offset`, `ObjectKey` | Each is a newtype whose invariant is documented and unconstructible-around — a value violating it is either impossible to build or returns an error; `Offset` arithmetic that would wrap returns an error rather than wrapping, so the monotonicity M3's sequencing rests on cannot be broken by a type-level accident; a property test per type asserts the **documented invariant cannot be produced** — ⚠️ not a constructor/accessor round trip, which at this point would constrain nothing, since no encoding exists until M2 and `oqueue-core` may take no workspace dependency to borrow one; no type names a concrete I/O type, a socket, or a clock, which is what lets every seam below be written in terms of them without acquiring I/O (NFR-51); `cargo test -p oqueue-core` passes. ⚠️ This row's justification is foundational, not behavioural — it cites NFR-51 because that is the criterion that can actually fail here, and deliberately claims none of the produce/fetch requirements these types will later carry | done |
| M0.6 | The error taxonomy in `oqueue-core`, with the redaction rules FR-44 will need | Errors follow `error-handling.md`; a redaction wrapper exists whose `Debug` **and** `Display` never reproduce the wrapped value; a test asserts that formatting an error carrying secret material yields no substring of that material (FR-44); `check-core-contract.sh` passes | done |
| M0.7 | **ADR-0003**: the four-crate leaf split (`buf`, `codec`, `checksum`, `index`) and the cross-crate inlining policy | ADR-0003 states either that every hot-path entry point crossing those boundaries carries `#[inline]`, or that the split is merged — and argues the rejected option's cost from doc 18 §0.4, which calls a non-generic `pub fn` without `#[inline]` "ruinous" for a byte-level codec split across rlib boundaries without LTO; it names both sides of the trade explicitly: **NFR-2**, the 10 ms cached tail read, where CPU work is the entire budget and a lost inline is paid on every batch, against **NFR-56**, whose pre-commit wall clock merging would raise by widening the rebuild unit and removing sideways parallelism — ⚠️ **not by changing DAG depth**, which stays 2 whichever way four siblings in a star are grouped (doc 19 §1.1–1.2); and it engages doc 19 §1.2's standing position that this exact tension "is resolved by thin LTO in release, not by merging crates", since an ADR that merges them is overturning a recorded assessment rather than choosing freely. It names what will check the runtime half once M2 has a hot path (`check-hot-path-bench.sh`, `performance.md` rule 18). ⚠️ M0 **decides** NFR-2's exposure here and measures nothing — M14 measures. Decided now, not when M2 is slow. ⚠️ **If ADR-0003 merges any of the four, it amends `architecture.md`'s crate table, this backlog's M0.8 row, and every "eleven crates" count in the same commit** — otherwise M0.8's acceptance is falsified by a decision made one row earlier, and the milestone carries two contradictory crate counts. Blocks M0.8 | done |
| M0.8 | The ten remaining crate skeletons | `buf`, `codec`, `checksum`, `index`, `store`, `coordinator`, `crypto`, `compact`, `broker` and `testkit` each exist with a `README.md` and an `AGENTS.md`; `#![forbid(unsafe_code)]` in every one but `buf`, `codec` and `checksum` (NFR-53); each depends on `oqueue-core` and on nothing else in the workspace except `broker`, which composes (NFR-52); `oqueue-testkit` is `publish = false` and appears in no runtime `[dependencies]`, only `[dev-dependencies]`, and its `README.md`/`AGENTS.md` say it holds harness and generators and **no fake** — ⚠️ `architecture.md`'s crate table still says `oqueue-testkit` holds "Fakes, generators, harness", which `contracts.md` rule 11 contradicts directly; **that cell is corrected in this commit**, because the crate's own documents and the crate map disagreeing is how the wrong convention gets re-established later; `check-readmes.sh` confirms every README's stated dependencies match its `Cargo.toml`; `cargo check --workspace` passes on both targets (NFR-40); every crate manifest carries `lints.workspace = true` — ⚠️ **the gate that enforces that, and the two other manifest assertions earlier reviews routed into `check-layering.sh`, are `M0.21`**, split out here because the checkpoint review found this row had quietly become ten crates plus three unrelated gate changes plus their negative tests, and `sdd.md` says split before writing code. ⚠️ `oqueue-crypto` is absent from doc 19's ten-crate layout and exists only in `architecture.md` — dropping it is an ADR, not a silent omission. ⚠️ Before the seams, not after: `contracts.md` rule 9 puts every fake in `oqueue-core`, but M0.11's *no-op* `KeyProvider` is production code and belongs in `oqueue-crypto`, which this row creates | done |
| M0.9 | The `Clock` seam, its fake **beside the trait in `oqueue-core`**, and **ADR-0004** | `Clock` is the only way to read time and nothing else in the workspace reads a real clock (NFR-51); the fake, with manual advance, lives in `oqueue-core` next to the trait — ⚠️ **not in `oqueue-testkit`**: `contracts.md` rule 9 and `testing.md` rule 4 both put it in core so every downstream crate is testable without a testkit dependency, and rule 11 calls a fake found in the testkit a layering violation to flag in review; a test asserting two `now()` calls with no advance between them return the same instant; `check-sans-io.sh` and `check-layering.sh` pass; **ADR-0004 is in this commit** and records what an implementor of `Clock` must guarantee — ⚠️ `contracts.md` rule 15 requires an ADR for a wholly new trait, and `check-core-contract.sh` enforces it mechanically: a new trait makes `old_traits.get(name)` `None`, which counts as a changed method set, so the commit is refused unless a file under `docs/internal/product/decisions/` is staged with it. An ADR in an *earlier* commit does not satisfy it — the gate reads `git diff --cached` | done |
| M0.10 | The `ObjectStore` seam, a **trivial** in-memory fake beside it in `oqueue-core`, and **ADR-0005** | The trait is `Send + Sync + fmt::Debug`, names no S3 or GCS type, and nothing outside it reaches object storage (NFR-51); it is expressed as ADR-0002 decided; the fake sits beside the trait in `oqueue-core` per `contracts.md` rule 9, **not** in `oqueue-store` — ⚠️ `oqueue-store`'s in-memory *implementation* (`testing.md` rule 6, `architecture.md`) is a real backend and is M1's, and this fake is what M1 task 5 replaces; a put/get round-trip test exercises the fake, which is what makes its existence observable in this commit — ⚠️ **and it drives the boxed future with `std::task::Waker::noop()` and a busy poll, taking no async runtime, not even under `[dev-dependencies]`**: `ADR-0002`, `ADR-0001` and `M0.16` all rest NFR-56's floor argument on no M0 task adding one, `check-layering.sh` never reads `[dev-dependencies]` so nothing would catch it, and until now this constraint lived only in `M0.4`'s commit message — ⚠️ `check-core-contract.sh` checks **implementors present and an ADR in the same commit** (`contracts.md` rule 14) and knows nothing about fakes, so the fake-per-trait property is asserted across the milestone by `m0-complete.sh` (M0.18). ADR-0005 is in this commit for the same rule-15 reason as M0.9. `check-core-contract.sh` and `check-sans-io.sh` pass. ⚠️ **Exactly one `ObjectStore` fake exists in the tree, and M1 rewrites this one rather than adding beside it** — two fakes with divergent conditional-write semantics is the highest-risk defect class in the project (`architecture.md`, doc 10 #33) | done |
| M0.11 | The `KeyProvider` seam, its fake in `oqueue-core`, a no-op provider in `oqueue-crypto`, and **ADR-0006** | The seam is wrap/unwrap only and deliberately **not** generate-data-key, because GCP Cloud KMS has no equivalent (doc 22 §4); nothing outside the seam touches key material (NFR-51), and neither implementation can print a wrapped key (FR-44); ⚠️ the **fake** lives beside the trait in `oqueue-core` (`contracts.md` rule 9) while the **no-op** provider — production code for the unencrypted path, not a test double — lives in `oqueue-crypto`, which M0.8 created; both are exercised by a test rather than merely present, so the encrypted and unencrypted paths never diverge (doc 22 §8); ⚠️ **those tests drive the boxed future with `Waker::noop()` and a busy poll, exactly as `M0.10` does** — `KeyProvider` takes the same shape as `ObjectStore` per ADR-0002, so it needs the same technique, and the no-async-dependency claim `M0.16`'s floor rests on is false the moment either row reaches for a runtime; ADR-0006 is in this commit for the same rule-15 reason as M0.9 and records that the seam's guarantee is wrap/unwrap and nothing more. ⚠️ **This row also closes `Error::SecretRejected`**, which `M0.6` added and nothing produces — `error-handling.md` rule 6 is against variants that cannot happen, so either `KeyProvider` returns it here or it is deleted here. And `security.md` rule 6 and FR-44 both still read literally against a secret reaching an error variant; `M0.6` amended rule 7 only, so whichever of the two outcomes lands, the text that bans it is amended or the variant goes; `check-core-contract.sh`, `check-layering.sh` and `check-sans-io.sh` pass | done |
| M0.12 | `bin/oqueue`: the composition root that starts, does nothing, and exits | `cargo run -p <bin>` exits 0 with a version line and no other side effect; it is the only place a concrete type is chosen (NFR-51, FR-50); `check-layering.sh` accepts it as a composer. ⚠️ `cargo build` links it on x86_64 only — aarch64 stays `cargo check` until a cross-linker exists, which is M13's work, and M0's completion condition must say so rather than claim a link it never performed | done |
| M0.13 | Allocator selection, wired with the profiling feature gate, and **ADR-0007** | ADR-0007 argues mimalloc, jemalloc, snmalloc and the system allocator with the rejected reasons (doc 18 §3.5, doc 10 #30) and states explicitly what the choice costs NFR-42, because `AGENTS.md` forbids adding a dependency that pulls in a C toolchain without recording why; ⚠️ it must also address doc 18 §3.5's jemalloc ARM page-size trap, where a binary built assuming 4 KB aborts at startup on 64 KB-page aarch64 kernels — NFR-40 makes aarch64 first-class, so this is a correctness question and not a footnote; the allocator is set in `bin/oqueue` and in no library crate; the heap-profiling allocator sits behind a non-default feature; `cargo check --workspace` passes with the feature off and on | done |
| M0.14 | Run every tree-scanning M-1 gate against the finished workspace and **record what each one actually inspected** | ⚠️ All five already *print* a count — `check-file-size.sh` "$checked .rs file(s) checked", `check-layering.sh` "manifest(s) hold", `check-sans-io.sh` "file(s) scanned", `check-unsafe.sh` "$total .rs file(s) scanned", `check-readmes.sh` "crate documents (${checked:-0} crate(s) checked)" — so adding the reporting is **not** the work and a row asking for it would close as a no-op. The work is that nobody has ever seen those numbers against Rust, and a matcher written for an imagined tree can return zero while the gate exits 0, leaving NFR-51, NFR-52 and NFR-53 unverified while reporting green. Acceptance: each gate is run on the completed workspace, its count recorded verbatim under "Notes on specific tasks" below, and every count is non-zero; any gate reporting zero is fixed here, and its fix is the commit's substance. ⚠️ If all five are already non-zero, **the recorded evidence is the deliverable**, and M0.18 is what stops it regressing. ⚠️ `check-core-contract.sh` is deliberately excluded: alone among the five it is scoped to `git diff --cached -- '*.rs'` and skips outright when no Rust is staged, so an "inspected count on this workspace" is not a thing it can report. The evidence that it fires on real Rust is M0.9, M0.10 and M0.11, each of which it refuses without an ADR staged beside the new trait | done |
| M0.15 | `check-coverage.sh`, and **measuring** NFR-55's constant | `cargo llvm-cov` reports line coverage **per crate**, and the gate fails on the *lowest* crate rather than on a workspace aggregate — ⚠️ NFR-55 and `testing.md` rule 19 both say **per-crate floor**, and a workspace-aggregate gate satisfies every other word of this criterion while letting one crate sit at zero behind the average; the threshold is a single literal in the script that no environment variable can move (`check-drift.sh` passes); the number is measured **on this workspace** and the measurement — command, output, date, and the per-crate breakdown — is recorded under "Notes on specific tasks" below. ⚠️ **Most of the eleven crates are empty skeletons at this point** — only `oqueue-core` (types, errors, three traits and their fakes) and `oqueue-crypto` (the no-op `KeyProvider`) hold executable code — so a naive per-crate minimum is 0 and enshrining it would satisfy this row and M0.18 while requiring nothing. The floor is derived from crates that contain **testable logic**; anything else is excluded by a **named list in the script carrying a reason per entry**, in the shape `check-file-size.sh`'s allowlist already uses, never silently — and an entry must be removed when its crate acquires logic. ⚠️ Two distinct kinds land in that list here and the rule must cover both, or the executor widens it silently: the empty skeletons, and **`bin/oqueue`**, which M0.12 gives a `main()` that starts and exits — a composition root by `check-layering.sh`'s own `COMPOSERS` set, whose whole job is choosing concrete types and which a per-crate floor would fail at 0% forever. ⚠️ If the code that exists is too little to support a defensible floor, **say so and record it** rather than writing down a number the workspace did not earn; that is a finding for M0's boundary review, not a 0 to enshrine. `check-coverage.sh` is wired into `.pre-commit-config.yaml` and CI and gains a `tests/gates/negative.sh` case proving it fails a crate below the floor — ⚠️ M-1's own standard is that a gate nothing invokes is a preference and a gate with no negative test is unproven, and M0 must not end having added two of them; `requirements.md`'s NFR-55 row loses its **UNDERIVED** marking and names the number, **and every other place that says the number cannot be chosen is corrected in the same commit** — ⚠️ **there are three, not one**: the NFR-55 row's status column, `requirements.md`'s own closing paragraph ("both NFR-55 and NFR-56 are constants that cannot be chosen until there is something to measure"), and `roadmap.md`'s "The numbers that do not exist yet" table. Striking the row and leaving the other two is the drift `M-1.47` and `M-1.52` were both created after the fact to repair, and no gate compares them. ⚠️ That paragraph also names the aggregate-throughput requirement, which stays underived and is M14's — correct the paragraph, do not delete it. ⚠️ Recording a number without measuring it here would make every later threshold argument rest on a fiction, and a guessed threshold is indistinguishable from a measured one a month later | done |
| M0.16 | `check-budget.sh` and **measuring** NFR-56's constant — closes `M-1.12` | The pre-commit suite's wall clock is measured on this workspace and recorded under "Notes on specific tasks" below with the command that produced it; the budget is a literal in the script; a suite over budget fails; timings are written as an artifact so erosion shows as a trend rather than as one sudden failure; ⚠️ the measured suite **includes `check-coverage.sh` and `check-crate.sh`**, both of which land before this row — a budget measured without them is a budget for a suite that no longer exists; `check-budget.sh` is itself wired into `.pre-commit-config.yaml` and CI and gains a `tests/gates/negative.sh` case proving an over-budget suite fails; the same three sites M0.15 names are corrected for NFR-56: the row's status column, `requirements.md`'s closing paragraph, and `roadmap.md`'s "The numbers that do not exist yet" table — including the paragraph above that table, which says M0 is one of the four milestones whose condition cannot be written, since M0 is then no longer one of them; `M-1.12`'s row is marked `done` against this commit. ⚠️ **The number is a floor, and the row must say so where it is recorded.** No M0 task adds an async dependency (ADR-0002 decides the runtime without taking it), so the suite is measured on a workspace that compiles none — and `M1`'s first one is compiled under exactly the dev and test profiles pre-commit builds, at `[profile.dev.package."*"] opt-level = 2`. Recording the measurement as steady state sets up a later breach of a budget non-negotiable 2 forbids raising. ⚠️ **So the row also fixes what happens at the first honest breach**, which is otherwise unwritten: the measurement is taken with `check-coverage.sh` and any mutation run already wired, and the script records the measured floor plus a stated headroom, with the rule that exceeding it is a task to make the suite faster or to move work to CI — never a raised literal. Without that, M1's first runtime dependency compiled under `opt-level = 2` leaves two exits and non-negotiable 2 forbids one of them. ⚠️ `milestones/M0.md` assumed the script already existed and that "only the number waits" — it does not exist, so this task writes both | done |
| M0.17 | Mutation testing wired with diff-narrowing: `scripts/mutants.sh` and `check-mutants.sh` | ⚠️ **Both paths are named by `testing.md` rules 16 and 17 and by the `tdd` skill, and neither exists** — this row creates them under those exact names, because a standard citing a script nobody wrote is the shape M0.1 is cleaning up elsewhere in this same milestone. `scripts/mutants.sh <crate>` runs only over what the staged diff touched, so cost is O(change) rather than O(workspace); a surviving mutant in changed code fails unless it is in `check-mutants.sh`'s baseline with a reason, which is a list nobody grows quietly (rule 17); the run's own wall clock is measured against NFR-56's budget. ⚠️ **Wired into `.pre-commit-config.yaml` or CI and given a `tests/gates/negative.sh` case** proving a deliberately weakened test lets a mutant survive and fails the gate — the same rule M0.15 and M0.16 apply to themselves, and it matters more here than for either of them: `testing.md` rule 15 and doc 19 make mutation testing the **primary** anti-slop gate, so a version of it that has never been observed to fail is the one gate whose silent uselessness would be least visible. ⚠️ If mutation testing does not fit the budget it belongs outside pre-commit and in CI — that is a decision to record, never a threshold to lower, and either way it is invoked by something | done |
| M0.18 | `scripts/gates/m0-complete.sh`, and its case in `tests/gates/negative.sh` | The gate asserts: the workspace checks on both targets and links on the host; **every `pub trait` in `oqueue-core` has a fake beside it in `oqueue-core`** — ⚠️ *not* in `oqueue-testkit`, which is what `milestones/M0.md`'s completion condition says and what `contracts.md` rules 9 and 11 forbid; that plan sentence also contradicts the plan's own Goal section, which says "a fake beside it"; `check-layering.sh`, `check-sans-io.sh` and `check-unsafe.sh` pass, because three of M0's requirements name exactly those three as their whole verification, and **all five gates M0.14 instruments report a non-zero inspected count**, `check-file-size.sh` and `check-readmes.sh` included, since a count nothing re-checks regresses the first time a matcher stops matching — ⚠️ `check-core-contract.sh` is not among them because it is diff-scoped and this gate runs standalone; **both** NFR-55's and NFR-56's constants are literals in their scripts rather than environment variables; **every gate M0 added — `check-crate.sh`, `check-coverage.sh`, `check-budget.sh` and the mutation-testing run — is invoked by `.pre-commit-config.yaml` or CI and has a case in `tests/gates/negative.sh`**, since M0 adds four gates and a gate nothing invokes is a preference; and `check-milestone-review.sh` covers every M0 commit. ⚠️ **and `m0-complete.sh` runs `tests/gates/negative.sh` itself**, because nothing else will: `m-1-complete.sh` runs it today as part of the *previous* milestone's condition, and neither the hooks nor CI do — so M0's four cargo fixtures, each of which was vacuously green at some point, would be exercised only by a finished milestone's gate. `tests/gates/negative.sh` gains a case proving `m0-complete.sh` fails on a broken workspace — M-1.46's standard, applied when the gate is written rather than thirty commits later | done |
| M0.20 | Act on the M0 checkpoint review: amend the rows it names, argue what has no row, and record why M0 exceeds the task cap | `reviews/` holds the verdict, staged; every major finding either names a row that now carries it (`M0.8`, `M0.10`, `M0.11`, `M0.16`, `M0.18`) or is argued in `baselines/review.txt`; `sdd.md`'s cap says the number is a heuristic and that exceeding it costs a recorded argument; `roadmap.md`'s deferral table names `fuzz.sh` and `check-secrets.sh` with receiving milestones; the five claims the review found false at HEAD are true; `check-milestone-review.sh` passes. ⚠️ Serves no FR/NFR — `sdd.md`'s outer-loop step, like `M0.0`'s opening one | done |
| M0.21 | The three manifest assertions in `check-layering.sh`, split out of `M0.8` | The gate rejects a crate manifest without `lints.workspace = true`, a **member** manifest carrying a `[profile]` section, and a root manifest whose `[profile.release]` lacks `overflow-checks = true` — ⚠️ the last is named by `security.md` rule 4 ("→ gate on the profile setting") and exists nowhere; each gets a `tests/gates/negative.sh` case; ⚠️ `check-layering.sh`'s header, its `name:` in `.pre-commit-config.yaml`, and its row in the standards tables are updated to say it checks manifests rather than layering alone, or `security.md` rule 4's gate ends up under a name nobody would grep for. ⚠️ **This row also widens `check-readmes.sh` to cover `bin/oqueue`**, whose `README.md` and `AGENTS.md` exist since `M0.12` and are checked by nothing — so `code-structure.md` rule 15's README-matches-manifest property is ungated for the one crate whose dependencies changed in `M0.13` and change again at `M1`. `M0.14` recorded it; recording it a third time would be the failure `M-1.47` names. ⚠️ Names no FR/NFR — it is gate work split out of `M0.8`, and `M0.0`'s criterion that every other row names one was true when `M0.0` closed | done |
| M0.22 | Act on M0's boundary review: record the verdict, write the rows it names, and schedule what has no row into the milestone that receives it | `reviews/` holds the verdict, staged; every major finding either names a row that now carries it (`M0.23`-`M0.26`) or is argued in `baselines/review.txt` against an obligation written into the receiving milestone's plan — ⚠️ an argued finding whose home is only a review artifact is the failure `M-1.47` names, so each argument cites a plan sentence that exists; `check-milestone-review.sh` passes. ⚠️ Serves no FR/NFR — `sdd.md`'s outer-loop step, like `M0.0`'s and `M0.20`'s | done |
| M0.23 | `check-drift.sh`'s name matcher sees every threshold M0 added | ⚠️ Non-negotiable 2 is unenforced for **both** of `check-budget.sh`'s constants: `THRESHOLD_RE` matches `COVERAGE_FLOOR` and neither `BUDGET_MS` nor `COMPILING_GATE_MS` — one commit after `M0.15` renamed its constant for exactly this reason and recorded that the name is load-bearing. The matcher covers both, or the constants are renamed, or both; ⚠️ **whichever is chosen, a gate must exist that fails when a threshold M0 added is invisible to `check-drift.sh`**, because "rename it" is a convention and the finding is that the convention was not followed on the next commit. `COMPILING_GATE_MS` decides whether NFR-56's budget applies at all and is asserted by nothing, anywhere — `m0-complete.sh` covers `BUDGET_MS` only. A `tests/gates/negative.sh` case for whatever is added. Serves NFR-55, NFR-56 | done |
| M0.24 | The two agent-facing index files are true at HEAD, and say where a missing script is scheduled | ⚠️ `M0.1` was written to correct exactly these two files and `M0` falsified them again: every `scripts/*.sh` any `SKILL.md` names now exists, while `.agents/skills/README.md` still says some are unwritten and tells an agent to report that a gate did not run. And both files say a missing script with no backlog row is *unscheduled* — wrong for both scripts that are actually missing, `fuzz.sh` and `check-secrets.sh`, which have no row and are scheduled in `roadmap.md`'s deferral table that neither file mentions. Acceptance: neither file claims a script is missing that is present; both name the deferral table as the second place to look; ⚠️ **and a gate checks the first half**, since `M0.1`'s acceptance said the same words and nothing has enforced them since. Serves no FR/NFR — it is `AGENTS.md`'s own accuracy | done |
| M0.25 | The claims M0 falsified and did not fix | Ten sentences are false at HEAD, in a milestone whose own convention is that the commit falsifying a sentence is the commit that fixes it: `Cargo.toml`'s `#[inline]` band stated as a floor against ADR-0003's table; `build.md` rule 5 carrying half of ADR-0003's correction; `clock.rs`'s "in program order" against ADR-0004 guarantee 5, which an M1 implementor would satisfy while violating the ADR; `performance.md`'s "full sharded run" against what `mutants.sh` does; the broken `[ADR-000n]:` reference-link definition **replicated into two more files after being recorded as a defect**; `oqueue-core/README.md`'s "No gate today" for a property `m0-complete.sh` asserts; `lib.rs` and `error.rs` narrating the seams as forthcoming; doc 19 contradicting itself and `architecture.md` on what `oqueue-testkit` holds; `.gitignore` naming target directories no script uses; and the hook count 15 in `requirements.md` and `roadmap.md` against 16 in `check-budget.sh` and `.pre-commit-config.yaml`. ⚠️ The hook count has a gate available and the rest do not — say which is which rather than fixing ten sentences and implying they stay fixed. Serves no FR/NFR | done |
| M0.26 | `m0-complete.sh` cannot report a skipped negative case as watched | ⚠️ The gate prints "every gate fails on a broken artifact" while the `check-coverage.sh` and `check-mutants.sh` cases sit behind `_have_llvm_cov`/`_have_mutants` and skip when the tool is absent — so on a machine without them, half of M0's four new gates are declared complete having never been watched to fail. That is the skip-is-a-pass shape **the same script refuses for `cargo`**, with the note "a skip here would report M0 complete having checked nothing". Acceptance: `tests/gates/negative.sh` reports how many cases skipped, `m0-complete.sh` fails when a gate it names in `M0_GATES` has a case that did not run, and a negative case proves it. Serves NFR-56's neighbours only indirectly — it is non-negotiable 4's `M0` instance | done |
| M0.27 | The minors M0 recorded against `check-drift.sh` and `m0-complete.sh` | ⚠️ **Harvested from commit bodies**, which is the collection step `review.md` rule 15 names no owner for — finding `bcf5d6f697f2` defers *the procedure* to M2, and doing it once by hand does not decide it. Fixes: `_ms` matches `_msg`, and the script has no suppression mechanism, so a legitimate `err_msg = env::var(...)` is refused with a remedy that misdiagnoses it; `m0-complete.sh`'s `THRESHOLD_RE` extraction has no `\|\| true` under `pipefail`, making its own fail branch unreachable and skipping sections 6-8; the hook-count extraction is line-oriented, so reflowing a 240-character line turns the gate red; the YAML fallback misses a block-sequence `stages:` and `YAMLError` escapes `except ImportError`; a `stages: [manual]` hook counts as invoked. Each gets a `tests/gates/negative.sh` case or a stated reason it cannot. Serves NFR-55, NFR-56 | done |
| M0.28 | `check-portability.sh` check 3's four sub-properties, each watched to fail | ⚠️ Three of the four delete cleanly with the whole negative suite green — the population split, the positive direction, and the inspected-nothing guard — which is the property `m0-complete.sh` itself calls untested. `M0.24`'s body named `M0.26` as their home and `M0.26` did not do it. Also: the claim patterns are four literal phrasings and the docstring implies the only limit is timing; `skip_case` records per *gate* where the row asked for a count, and its argument is an unvalidated bare filename nothing checks against `M0_GATES`; and two comment paragraphs are orphaned from what they document. Serves no FR/NFR — it is non-negotiable 4's own coverage | done |
| M0.29 | Every documentation minor M0 recorded and did not fix | ⚠️ **Bounded by what is written down**, not by a fresh audit: the `Recorded, not fixed` sections of M0's commit bodies plus the two boundary-review artifacts. `review.md`'s "four lines below" is fourteen and was replicated into two more documents; `roadmap.md` says 31 minors where two other files say roughly thirty, and carries a doubled word; rule 15's interim sentence is in neither skill that loads at review time, which `M0.19`'s acceptance required; `AGENTS.md` explains a missing script by an M-1 row `M0.16` closed; `.agents/skills/README.md` says seven standards where there are fourteen; `backlog.md` quotes `THRESHOLD_RE`'s pre-`M0.23` value; `build.md` rule 7 says gating `release` gates all four profiles, which is a property of today's manifest and not of the gate; three rustdoc bodies still render a literal `[ADR-000n]`; `testing.md` still calls the full run sharded; `bin/oqueue`'s manifest still names `MALLOC_CONF`. ⚠️ Each is either fixed or given a stated reason it stays. Serves no FR/NFR | done |
| M0.30 | The milestone-boundary gate tier survives the milestone that wrote it | ⚠️ `tests/gates/negative.sh` and `check-milestone-review.sh` are invoked by **nothing** in `.pre-commit-config.yaml` or `gates.yml` — their only callers are `m-1-complete.sh` and `m0-complete.sh`, which are themselves invoked by nothing and which the `milestone` skill runs only while their own milestone is current. So the day M0's verdict lands, the project's proof that its gates can fail has zero invokers. ⚠️ **And the rule underneath it — "a gate nobody has watched fail is a gate nobody has tested" — is in no standard**: `grep -rin "watched to fail" docs/internal/standards/` returns nothing, and it lives only in backlog acceptance rows and one script header. Acceptance: **CI invokes `tests/gates/negative.sh` on every push**, and `testing.md` states the rule and names the suite. ⚠️ **`check-milestone-review.sh` is deliberately excluded, and this clause was amended to say so** rather than argued past in the notes: it is a *completion* gate that is red for most of a milestone's life by design, so a per-push step would be red on a tree where every other gate passes — review measured it at 4 of 31 the moment it was wired. `reviews/README.md`'s claim that CI *can* check milestone coverage stays a capability claim and is not made into a wired step here; doing that needs a gate that knows what a **closed** milestone is, which nothing does. Serves NFR-50 | done |
| M0.31 | Two documents asserting a defect that is not there, and an invariant that is | ⚠️ `oqueue-broker/README.md`'s Invariants row says the crate "names no concrete backend, clock or **socket** type" — in the same file, below its own "the sockets have to be somewhere. This crate is that somewhere", and beside the `check-sans-io.sh` exemption that exists to permit them. `architecture.md` names three seams and none is a socket, so the row cannot be read as scoped to seam implementations; the first commit that writes the connection loop falsifies it, and the row's own cell says review is the only holder. ⚠️ And `M1.md` routes "ADR-0005's FR-50 citation where FR-31 is the requirement" — but ADR-0005 disclaims FR-31 deliberately, in a paragraph of its own, so an M1 executor following that bullet edits a correct disclaimer into a contradiction. ⚠️ The citation is still one of the five survivors `M0.29` counted and `M1.md` now enumerates — its fix is to drop the parenthesis or cite NFR-51, not to write FR-31. Serves no FR/NFR | done |
| M0.32 | The three prose defects M0's closing review found, and a cap argument that covers them | ⚠️ Three sentences are false at HEAD and none is recorded anywhere: `M1.md` routes `SKIPPED_COUNT` as "read by nothing repo-wide" when `M0.30` wired CI to read it one commit later — **an executor following that bullet breaks CI**, which is exactly the shape `M0.31` was written for, recurring in the same section one commit on; `backlog.md`'s cap argument says 30 rows against a table that has grown past it, and stops enumerating at `M0.29`, having declared that a sixth bucket is the unqualified growth `sdd.md` forbids; and `gates.yml` says `reviews/README.md`'s CI claim "stays untrue, and `M0.30`'s notes say so" while those notes say the opposite. ⚠️ **And the five minors `M0.30` and `M0.31` recorded get rows or a routed home**, one of them promised "a row in the next commit" by its own commit body — `review.md` rule 15's interim instruction, unfollowed by the two commits after the one that mirrored it into both skills. Acceptance: each of the three sentences is true; the cap argument covers through this row and is re-argued rather than renumbered; the five have a home a plan reads. Serves no FR/NFR | done |
| M0.19 | Two rules in `review.md` that bound the loop: a claim about tool behaviour is measured before it is written, and a `minor` on a `pass` is recorded rather than re-reviewed | `review.md` gains both; `which-standards.sh` selects `review.md` for a shell script and a research document, which is where two of the three worked examples lived; the `review` and `milestone` skills that duplicate the resolution rules agree with rule 15, including that nothing is staged for a minor. ⚠️ Comment-density, criterion-length and ADR-length rules were cut from this task — no gate, and the criterion-length one made the 20-task cap binding in the commit that filled slot 20. ⚠️ Names no FR/NFR; `M0.0`'s criterion that every other row does was true when `M0.0` closed | done |

⚠️ **M0.15 through M0.18 are not blocked by NFR-55 and NFR-56 being
UNDERIVED, and `next-task`'s step 3 must not be read as saying they are.**
That rule skips a task that *depends on* an underived requirement — one
that cannot proceed until somebody supplies a number. M0.15 and M0.16 are
the tasks that **produce** the two numbers, and a task blocked by its own
output strands the milestone at exactly the point it was created to
unblock; M-1 spent its whole life with both constants underived precisely
so M0 could measure them. ⚠️ M0.17 and M0.18 genuinely do *consume* those
constants, so they are blocked in the ordinary way — **by M0.15 and M0.16
being unfinished, not by `requirements.md` carrying an UNDERIVED marking**.
The distinction matters if M0.16 stalls: the answer is to finish M0.16, not
to treat the marking as a reason to skip past it.

### Notes on M0

Two kinds of note live here, and they are separated because they are read for
different reasons. **Where this decomposition diverged from the plan** is
written once, when M0 opened, and is never updated afterwards — it is evidence
about planning horizon. **Notes on specific tasks** grow as tasks land, and are
where M0.14's gate counts and M0.15's and M0.16's measurements are recorded:
the command, its output, and the date, so anything those tasks fix can be
re-derived by someone who doubts it rather than taken on trust.

#### Where this decomposition diverged from the plan

⚠️ These record where the task table above departs from
[`milestones/M0.md`](milestones/M0.md)'s eighteen provisional items. A plan item
that turned out to be wrong is not a spec that turned out to be wrong and needs
none of that ceremony — nothing was committed to it. It is written down because
`sdd.md` asks for it as evidence about how far ahead this project can usefully
see, not as a correction.

The mapping first, so the claims after it can be checked rather than believed:

| Plan item | Became | Changed? |
|---|---|---|
| 1 workspace `Cargo.toml` | M0.2 | merged with item 3 |
| 2 five build profiles | M0.3 | gained ADR-0001 |
| 3 `oqueue-core` skeleton | M0.2 | merged with item 1 |
| 4 `Clock` seam + fake | M0.9 | gained ADR-0004; fake relocated to `oqueue-core` |
| 5 `ObjectStore` seam + fake | M0.10 | gained ADR-0005; fake relocated to `oqueue-core` |
| 6 `KeyProvider` seam + fakes | M0.11 | gained ADR-0006; fake and no-op split across two crates |
| 7 core types and IDs | M0.5 | — |
| 8 error taxonomy | M0.6 | — |
| 9 nine crate skeletons | M0.8 | merged with item 10, so ten |
| 10 `oqueue-testkit` | M0.8 | merged with item 9; holds no fake |
| 11 `#![forbid(unsafe_code)]` everywhere | M0.2 + M0.8 | dissolved |
| 12 `check-layering.sh` vs real crates | M0.14 | merged with 13, restated |
| 13 `check-sans-io.sh` likewise | M0.14 | merged with 12, restated |
| 14 allocator + profiling gate | M0.13 | gained ADR-0007 |
| 15 `check-coverage.sh` + NFR-55 | M0.15 | — |
| 16 `check-budget.sh`'s constant | M0.16 | premise wrong |
| 17 mutation testing | M0.17 | — |
| 18 `bin/oqueue` | M0.12 | — |
| — | M0.0 | no plan item: opening the milestone |
| — | M0.1 | no plan item: correcting M-1's stale bootstrap notes |
| — | M0.4 | no plan item: ADR-0002, the async runtime |
| — | M0.7 | no plan item: ADR-0003, the leaf split |
| — | M0.18 | no plan item: M0's own completion gate |

Thirteen of the eighteen changed, and five tasks answer to no plan item. The
reasons:

1. **The plan puts the fakes in the wrong crate, and its own Goal section
   disagrees with its completion condition.** The condition says "every `pub
   trait` in `oqueue-core` has a fake in `oqueue-testkit`"; the Goal two
   sections earlier says "every trait seam in `oqueue-core` has a fake beside
   it". The Goal is right and the condition is wrong: `contracts.md` rule 9 and
   `testing.md` rule 4 both put every fake beside its trait in `oqueue-core`,
   precisely so a downstream crate is testable without a testkit dependency,
   and `contracts.md` rule 11 calls a fake found in `oqueue-testkit` a layering
   violation to flag in review. A standard beats a plan, so M0.9, M0.10 and
   M0.11 put their fakes in core, `oqueue-testkit` lands in M0.8 as a skeleton
   holding no fake, and M0.18's gate asserts the standard's location rather
   than the plan's. ⚠️ This is the divergence with the most reach: had it gone
   the plan's way, eleven crates would have been built around a layering
   violation that no script checks for.
2. **That relocation is also why the crate skeletons come before the seams.**
   M0.11's *no-op* `KeyProvider` is production code for the unencrypted path,
   not a test double, so rule 9 does not apply to it and it belongs in
   `oqueue-crypto` — which means the crate has to exist first. The plan
   ordered the seams before the skeletons, which works only if every
   implementation is a fake.
3. **The plan lists no ADR tasks, but names four decisions that must be made
   before M0's code.** Seven ADRs are in the list — four for those decisions,
   and three more that `contracts.md` rule 15 forces, one per new
   `oqueue-core` trait (M0.9, M0.10, M0.11): a wholly new trait defines what an
   implementor must guarantee, and `check-core-contract.sh` refuses the commit
   without a `decisions/` file staged beside it. Those three have no discretion
   in their placement. The other four are placed by one test: **could the
   answer change what the next task *is*?** If so the ADR stands alone, because
   a decision reached inside the commit that depends on it is a decision made
   under pressure to keep the diff small. M0.4 decides how an async seam is
   expressed, which decides what M0.9, M0.10 and M0.11 are; M0.7 decides
   whether M0.8 creates ten crates or fewer. If not, the ADR travels with the
   change it governs, where its consequence is in the same diff and can be
   checked against it: the root manifest gets profiles either way (M0.3) and
   the binary sets some allocator either way (M0.13) — those ADRs record *why
   these values*, not *what to build*. ⚠️ Note this is **not** "how many later
   tasks does it block" — M0.7 blocks only M0.8 and still stands alone, because
   blocking one task whose shape it decides is the case that matters. ⚠️ The
   ADR numbers run in execution order: 0001 profiles, 0002 the async seam, 0003
   the leaf split, 0004–0006 the three seams, 0007 the allocator.
4. **Plan items 1 and 3 merge, and `scripts/check-crate.sh` joins them.** A
   workspace manifest with no members compiles nothing, so "the tree is green"
   would be vacuously true of a commit holding only item 1. M0.2 pairs the root
   manifest with `oqueue-core` — the smallest tree that actually compiles — and
   item 2's profiles stay their own task because they carry ADR-0001 and nothing
   depends on them landing first. ⚠️ **No plan anywhere names `check-crate.sh`**,
   yet the `tdd` skill's definition of done and the `milestone` skill's loop both
   invoke it and doc 19 §5 specifies it. It does not exist, and neither does any
   pre-commit hook that runs a cargo command — so the plan as written has M0
   introducing eleven crates of Rust that no gate compiles, formats, lints or
   tests. It lands in M0.2 rather than as a twentieth task because a commit that
   adds the first Rust and a commit that makes Rust checkable are the same
   change; splitting them means deliberately committing code nothing checks.
5. **Plan items 9 and 10 merge.** With no fake in it, `oqueue-testkit` in M0 is
   a skeleton exactly like the other nine: `Cargo.toml`, `README.md`,
   `AGENTS.md`, `publish = false`. A commit creating one empty crate is not a
   task, and the fake-per-trait convention item 10 wanted enforced turns out to
   be enforced somewhere else entirely — in core, by M0.18's gate.
6. **Plan item 11 dissolves rather than becoming a task.** "`#![forbid(unsafe_code)]`
   everywhere except `buf`, `codec`, `checksum`" is one attribute line per crate
   and cannot be a commit of its own without every crate first existing without
   it — which would mean deliberately committing a tree `check-unsafe.sh` is
   meant to reject. The attribute lands with each crate, in M0.2 and M0.8, and
   both rows carry it in their acceptance.
7. **Plan items 12 and 13 became one task, restated twice.** "Run
   `check-layering.sh` against real crates; fix what it gets wrong" has no
   checkable acceptance when nothing turns out to be wrong. The first restating
   asked each gate to *report* what it inspected — which turned out to be
   already true of all five, making the row a no-op. What is left, and what
   M0.14 asks for, is that the numbers be **seen and recorded**: a matcher
   written against an imagined tree can return zero while the gate exits 0, and
   three requirements name these gates as their entire verification.
8. **Plan item 16 is wrong about `check-budget.sh` already existing.** It says
   "the script is M-1.12; only the number waits for this milestone". `M-1.12` is
   the one M-1 row that never landed, so no script exists and M0.16 writes both
   the script and the constant, then closes `M-1.12`.
9. **The plan lists no task for M0's own completion gate.** It states the
   completion condition, which is not the same as writing the script that
   asserts it — M-1 learned this twice (`M-1.16` wrote the gate, `M-1.46` had to
   come back and write its negative test). M0.18 does both at once.
10. **M0.1 has no plan item because the plan could not have known.** Declaring
    M-1 complete makes `AGENTS.md`'s and `.agents/skills/README.md`'s "these
    gates do not exist yet" notes false, and they are the first thing an agent
    reads in any session. It was found by M0.0's own review.

Two things the plan asserts that this decomposition deliberately does **not**
widen to match. The M0.10 and M0.11 rows cite NFR-51 — the seam exists and
nothing outside it does I/O — rather than FR-31 and FR-41, which are about
behaviour those fakes do not have: M1 writes the S3 and GCS backends, M8 writes
the KMS providers. ⚠️ The plan calls the `ObjectStore` fake **trivial** in as
many words (item 5); it gives no such instruction for `KeyProvider` (item 6), so
that half of the argument rests only on the second ground, which is the
sufficient one anyway. Claiming FR-31 here would leave M1 looking like a
refinement of a requirement already served.

#### Notes on specific tasks

⚠️ **M0.14's gate counts, M0.15's coverage measurement and M0.16's pre-commit
timing belong here**, each with the command that produced it and the date,
because two of those tasks strike an **UNDERIVED** marking off
`requirements.md` and a constant recorded without its derivation is
indistinguishable from a guessed one a month later.

**M0.2** ran the five tree-scanning gates against Rust for the first time.
Every one passed and every one reported a non-zero count, so `M0.14`'s real
question — whether a matcher written against an imagined tree matches a real one
— has a first data point rather than an assumption: `check-layering.sh` 1
manifest, `check-sans-io.sh` 1 file, `check-unsafe.sh` 1 file, `check-file-size.sh`
1 file, `check-readmes.sh` 1 crate. ⚠️ Three of them *skipped* before
`git add`: they read `git ls-files`, so a file that exists but is untracked is
invisible to them. Worth knowing before reading a green run as evidence.

Two decisions the plan did not anticipate. **Resolver 3, not the 2 the plan
names**, and the line is load-bearing rather than explicit: edition 2024 implies
resolver 3 for a *package* manifest, but the root here is a **virtual** manifest
with no `[package]` and therefore no edition to imply anything. Deleted, cargo
warns and falls back to resolver 1 — losing feature unification and MSRV-aware
selection, with no gate catching it. The first version of this note said edition
2024 made 3 the default; review disproved it by deleting the line. And **`--locked` on the hook, not only in CI**: `build.md`
rule 2 puts it in CI, doc 21 §9 requires the hook and CI to run the same script,
and the two together leave no version of `check-crate.sh` that has it in one
place and not the other.

The negative tests for `check-crate.sh` cost more than the script did, and the
reason is worth recording. Adding `--locked` silently broke all three fixtures:
with no `Cargo.lock` in the scratch workspace, every one of them failed on the
missing lock rather than on the defect it was written to plant. **They still
went red, so the suite still reported green** — the exact shape of a test that
has stopped constraining anything. The first fix, generating a lockfile in the
shared setup, failed too, because `cargo generate-lockfile` needs a valid
package and one whose `src/lib.rs` does not exist yet is not valid; `|| true`
swallowed it and the fixtures were unchanged. Neither was visible by reading.
Both were found by mutant testing — disabling the fmt check and observing that
the unformatted fixture *still failed* — and the setup now hard-fails rather
than continuing without a lockfile. A fourth case was added for `--locked`
itself, and all four were re-verified one mutant at a time: each fixture passes
wrongly when, and only when, its own check is disabled.

**M0.1** removed three stale claims and wrote no list to replace them. The
tempting correction was "these five scripts are still missing"; the row forbade
it, and the reason is worth keeping. `AGENTS.md` is layer 0 — loaded into every
session before anything else — so a list there is read more often than any
other statement in the repository and is updated least often, which is the
worst possible ratio. Five of the scripts such a list would name land during M0
itself, so it would have been wrong by M0.2 and stayed wrong for sixteen tasks.
`M-1.47`, `M-1.49` and `M-1.52` are three commits spent chasing exactly that
shape before the fix turned out to be deleting the list rather than refreshing
it a fourth time.

Two things did have to be stated rather than delegated, because `backlog.md`
does not answer them. The first is the scope of the claim: M-1 completing means
**six of the seven non-negotiables** each have a passing script — rule 3 has
none and cannot — and not that every gate the standards mention exists — `security.md` alone names two that do not, and
neither is scheduled anywhere. The second is that CI and a local clone are
not in the same state, and neither covers everything: `.github/workflows/gates.yml`
runs `pre-commit run --all-files` on every push but with `SKIP: check-reviewed`,
because the verdict artifact lives under gitignored `target/` and CI has nothing
to read — so **rule 4 is enforced by the local hook alone** — while a fresh clone
enforces nothing at all until someone runs `pre-commit install` once. The old
sentence said `.git/hooks` "is not tracked, so a fresh clone runs none of these"
and pointed at M-1.14 as the fix; M-1.14 landed, and made half of that false
while leaving the other half true. ⚠️ Writing "so nothing merges unchecked" was
the first attempt at the replacement and was caught in review: it is the same
overstatement this task exists to delete, one sentence after deleting it.

**M0.17** wired mutation testing, and the first full run found eleven surviving
mutants in `oqueue-core` — **all eleven killed by writing tests, none argued
into the baseline.** That is the ratio to keep, and `baselines/mutants.txt`
starts empty saying so.

What survived is the interesting part, because coverage was already at 91.63%
and `check-coverage.sh` was green: five `Display` impls nothing rendered, four
accessors that could return a wrong value undetected — two replaced by
`"xyzzy"` and two by `0` or `1` — and two `<` guards that could become `<=`
because no test used **zero**. ⚠️ `M0.5`'s property tests
assert what *cannot be produced* — the invariant — and were right to; they
simply do not assert that an accessor returns what went in. Both kinds are
needed, which is `testing.md` rule 15's point stated as a measurement rather
than as advice.

⚠️ **Narrowed in pre-commit, full in CI** — both halves of `testing.md` rule 16,
and no conflict with NFR-56 after all. The first version put it in CI only, on
the reasoning that a run costs ~28 s against a 10 s budget. ⚠️ **That number was
measured in the wrong mode.** 23 s is the *unnarrowed* run over `oqueue-core`;
narrowed, `--in-diff` generates mutants only for touched lines and the same
tool is **4.8 s warm for a 13-mutant diff**. ⚠️ **A second wrong number was
recorded before that one stuck**: 0.08 s, which is the early exit taken when a
diff produces no mutants at all — a skip, not a run, and this commit's own diff
takes it because it stages only test files. The number that decides placement
leaves ~5 s of NFR-56's 10 s budget against a suite using 2.2 s. Cold, the same
diff is 9.7 s and does not fit; `check-budget.sh` reports that as a compiling
run rather than as erosion.

⚠️ The cost is proportional to the change, so a very large diff can still be
slow, and `check-budget.sh` is what notices: a suite permanently reported as
`compiling` is the signal that this belongs back in CI only.

Two vacuous paths were found while testing the gate itself. A narrowed run over
a diff touching only tests generates **no mutants at all**, and the gate
reported `ok ... 0 survivor(s), all argued` — a pass for a run that never
happened; it now skips and says the commit is unproven. And `check-mutants.sh`
called `mutants.sh` bare under `set -e`, so a surviving mutant (exit 2) killed
the gate *before* it read the exit code: it exited 2 with no output and the
negative suite counted that as a correct failure. ⚠️ Both are the shape
`tests/gates/negative.sh`'s own header documents, found in a gate written by
someone who had read it.

**M0.16** measured NFR-56 and closed `M-1.12`, which had been open since M-1
waiting for a workspace. Command, verbatim, on 2026-08-16 at commit `2bcf6f6`:

```
pre-commit run --all-files          # warm cache
```

**2.27 s wall clock across 14 hooks**, of which `check-coverage.sh` was ~1.0 s
and `check-crate.sh` ~0.47 s — ⚠️ both in the suite, which this row's acceptance
requires, since a budget measured without them is a budget for a suite that no
longer exists.

**The budget is 10 s.** Not 2.3 s: ten seconds is roughly where a developer
stops waiting and switches context, so it is the number that means something
rather than the number that was observed. ⚠️ And it is a **floor** — the
workspace compiles no async runtime, because ADR-0002 chose Tokio without taking
the dependency and no M0 task adds one. `M1` adds both a runtime and a cloud
SDK under `[profile.dev.package."*"] opt-level = 2`.

⚠️ **The rule: take out every gate over 5 s; if what remains is under budget,
the run was a build and is not measured against it.** A cold `target/` puts this
suite at 28.6 s with `check-crate.sh` alone at 21.6 s, and there are **two**
independent from-scratch cargo builds in it — `check-crate.sh` and
`check-coverage.sh`'s instrumented one — so a rule tolerating only the largest
fails a genuine cold clone.

⚠️ Three rules were tried and two were rejected **by measurement**, which is the
part worth keeping: `slowest > 5 s` alone would have made this gate unfailable
from `M0.17` onward, since a mutation run is minutes; and "the slowest takes a
strict majority" still let 21.6 s of build hide 19.9 s of real erosion. Five
shapes are tabulated in the script.

⚠️ **The residue, stated rather than hidden:** a gate individually over 5 s is
never counted, so two gates at 5.1 s each read as two small builds rather than
as an eroded suite. That is the price of a single-run heuristic, and the trend
is the mitigation — a `suite.tsv` that is permanently `compiling` is itself the
signal, and ⚠️ ~~nothing reads it yet~~ — **`M1.30` made the exemption branch
read it back**, reporting when the rate crosses a quarter of recorded runs;
whether a persistently compiling suite should *fail* is `M1.40`.

⚠️ **The gate does not run the suite**, which would double the thing it exists
to keep short. `lib.sh`'s `finish()` records each gate's own wall clock and
`check-budget.sh` adds up the entries for the current run. The grouping key is
the **process group**: measured first, because `pre-commit` gives each hook a
different `PPID` and the same `PGID`, and an earlier design keyed on `PPID`
would have seen one gate per group. Two artifacts: `target/timings/gates.tsv`
holds per-gate rows and is **consumed** by this gate each run (aged at 1 day, so
an abandoned run's rows cannot be summed into somebody else's — ⚠️ **by the
ageing, not by the PGID**, which is the part worth being precise about: the
prune runs *after* the sum, so a live run is separated from a concurrent one by
its group id, and an abandoned run's rows are separated from a **later** run
that the kernel gave the same recycled group id by their age. Without the
ageing those rows are immortal and the next suite in that group adds seconds it
never spent), and
`target/timings/suite.tsv` is the 30-day trend, one row per suite run written
once the verdict is known, statused `ok`, `over`, or `compiling:<gate>` — so a
failing run, an exempted run and a clean one are three distinguishable things
rather than two.

**M0.15** measured NFR-55. Command, verbatim, on 2026-08-16 at commit
`f79f422`:

```
cargo llvm-cov --workspace --summary-only --locked
```

Per crate, line coverage — ⚠️ rolled up from `llvm-cov`'s per-*file* rows, since
it reports files rather than crates:

```
oqueue-core     197/215 lines   91.63%
oqueue-crypto     19/19  lines  100.00%
bin/oqueue        30/31  lines   96.77%
(nine other crates)  0 executable lines — no llvm-cov rows at all
```

**The floor is 85%**, deliberately below the 91.63% measured. A floor equal to
the measurement fails on the next commit that adds a line before its test, and
the only remedies then are lowering it — which non-negotiable 2 forbids — or
padding tests. 85% leaves ~6.6 points while still refusing a crate that is
meaningfully untested.

⚠️ **Ten of twelve crates are excluded by name, each with a reason**, which is
the number worth staring at: nine empty skeletons and `bin/oqueue`. A per-crate
minimum taken naively across all twelve would be 0 or undefined, and enshrining
that would satisfy this row while requiring nothing. **Each skeleton entry is
removed by the milestone that fills its crate** — an exclusion outliving its
reason is this list's whole risk.

⚠️ **The exclusion list is two lists, because the two kinds expire
differently.** `UNTIL_FILLED` holds the nine skeletons and asserts they stay
empty — the gate **fails** if one acquires executable lines without its entry
being deleted, which is the hazard `check-file-size.sh`'s allowlist does not
carry: a stale entry there is inert until a file grows, a stale entry here
switches a floor off exactly when the crate starts to matter. `ALWAYS` holds
`bin/oqueue` alone, permanently, and expects it to have code. Review found the
first version silently dropped 215 lines of `oqueue-core` when its name was
added to the single list.

⚠️ **The constant is called `COVERAGE_FLOOR`, and the name is load-bearing.**
`check-drift.sh` matched, **on the day `M0.15` closed**, on
`threshold|_limit|_budget|_ceiling|_floor` — ⚠️ `M0.23` and `M0.27` have since
widened it, and the live value is in that script; the
first version called it `MIN_CRATE_COVERAGE`, which matches none of them — so it
could be made environment-settable and `check-drift.sh` passed anyway. The
acceptance clause "(`check-drift.sh` passes)" was being satisfied by a gate that
never looked at the constant. Measured by review.

⚠️ **The gate enumerates crates from `cargo metadata`, not from the coverage
report**, and that was a bug found while testing it: a crate with no executable
lines produces *no* `llvm-cov` rows, so iterating the report silently ignored
exactly the crates the exclusion list exists to make deliberate — the gate
reported "1 excluded" while ten were listed, and its "not in the exclusion list"
branch could never fire.

**M0.14** ran the five tree-scanning gates against the finished workspace and
recorded what each inspected. Verbatim, on 2026-08-16, at commit `05b3b08`,
each produced by `./scripts/<name>.sh` with no arguments from the repository
root — ⚠️ the command matters here because `M0.15` and `M0.16` record numbers
that become immovable literals, and a measurement nobody can re-derive is one
nobody can challenge:

```
check-layering.sh    ok  crate layering (12 manifest(s) hold)
check-sans-io.sh     ok  no library crate touches a socket, the real clock, or
                         object storage (28 file(s) scanned)
check-unsafe.sh      ok  unsafe budget holds (31 .rs file(s) scanned,
                         0 baselined SAFETY block(s))
check-file-size.sh   ok  file size (31 .rs file(s) checked)
check-readmes.sh     ok  crate documents (11 crate(s) checked)
```

**Every count is non-zero**, which is the question the row was written to answer:
a matcher written against an imagined tree can return zero while the gate exits
0, and none of the five does. No gate needed fixing, so the evidence is the
deliverable and `M0.18` is what stops it regressing.

⚠️ **The counts also make two scope gaps visible. One is by design; one is a
defect, now scheduled into `M0.21`.** The
tree holds 31 tracked `.rs` files — 28 under `crates/`, 3 under `bin/` — and 12
crate manifests.

- `check-unsafe.sh` and `check-file-size.sh` see **31**: the whole tree.
- `check-sans-io.sh` sees **28**: `crates/` only, because `bin/oqueue` is where
  choosing a concrete socket, clock or backend is the *job*. Correct by design.
  ⚠️ It does **not** mean the "only place a concrete type is chosen" property is
  unheld: ⚠️ **the two exemptions are not the same size**, which every version of
  this sentence has got wrong. `oqueue-broker` is exempt from all three
  patterns; `crates/oqueue-store` is exempt from `STORE_RE` **only** and is
  still scanned for sockets and for the real clock. So the crate count depends
  on which pattern is being asked about, and no single number is right for the
  gate as a whole — read the `BROKER_DIR`/`STORE_DIR` constants near the top of
  `scripts/check-sans-io.sh` **and** the branch that uses them, which are not
  adjacent. `M0.29` first wrote nine, then ten, then nine again, each time
  from a different wrong derivation. What excluding `bin/` leaves ungated is business
  logic migrating *into* the composition root, which is a different claim and is
  review's.
- `check-layering.sh` sees **12**: every crate plus `bin/oqueue`, which it
  treats as a composer.
- `check-readmes.sh` saw **11**: `crates/` only. ⚠️ **That one was a defect**,
  and `M0.21` closed it — the glob is `crates/*/` and `bin/*/` now.
  ⚠️ **The census above still reads 11, and must**: it is introduced as
  verbatim output on 2026-08-16 at commit `05b3b08`, so re-deriving it would
  destroy the `M0.14` record rather than update it. A live count comes from
  running the gate. `bin/oqueue` had a `README.md` and an `AGENTS.md`
  as of `M0.12` and neither was checked, so `code-structure.md` rule 15's
  README-matches-manifest property was ungated for the crate whose dependency
  list changed in `M0.13` — and changes again when `M1` adds a runtime and a
  storage SDK to the composition root. ⚠️ **Recorded twice before it was
  scheduled** (`M0.12`'s commit body named `M0.13` as the commit that would make
  it bite, and it did), which is the signal that prose is the wrong place for a
  defect. ⚠️ **Two of the census lines are now text no run reproduces**, for the same
  reason: `check-readmes.sh` reports 12 today, and `crate layering (12
  manifest(s) hold)` became `crate manifests (12 manifest(s) hold)` when
  `M0.21` renamed that gate's output. Both are correct as a dated record and
  wrong as a current one.

**M0.21** fixed a sticky-section bug in the manifest parser it added, and
⚠️ **left the same bug in the three parsers around it**, deliberately, because
fixing them is not what the row asked for: `package_name()` and
`runtime_deps()` in the same file, and ⚠️ **`check-readmes.sh`'s, which is the
parser this very commit widened to `bin/*/`**. `sections()` now ends a table on any
line starting with `[`, including a header it cannot name; `package_name()` and
`runtime_deps()` still use `^\[([A-Za-z0-9_.\-]+)\]$` and *skip* what does not
match, carrying the previous section forward. The reachable shape is a quoted or
`cfg`-gated table — `[target."cfg(unix)".dependencies]`, which nothing in this
workspace writes today and `M1` plausibly will. Direction matters: for the star
topology it mostly fails *closed* (keys of an unparseable table get read as the
previous section's dependencies, producing a false violation), but a runtime
dependency declared only under a `[target...]` table is invisible to it, which
is a **silent pass on NFR-52**. One helper now exists that gets this right;
routing the other two through it is a small change and is **for M0's boundary
review to schedule**, not for the row that happened to notice. ⚠️ **Prose is
the wrong place for a defect** — this section says so eight lines down — and
the only thing that makes it acceptable here is that the boundary review is
the *next* action after `M0.18`, not a distant one. If `M0` closes without a
row for this, that is the failure `M-1.47` names, happening again.

**M0.18** found two things by being run, which is the argument for writing a
completion gate rather than a completion checklist.

- ⚠️ **Two M0 gates were reported as never watched to fail, and both had
  cases.** The gate looked for `^run_case`; the suite's calls are indented
  inside its dispatch block. A gate asserting a property of a *test suite* got
  the answer wrong on its first run, which is the same class of defect as a
  matcher returning zero — and the reason `M0.14`'s counts are re-checked here
  rather than trusted.
- ⚠️ **The aarch64 wall is not where `M0.md` says it is.** The plan predicted a
  missing cross-*linker*. Measured: `cargo check` for aarch64 fails on
  `bin/oqueue` before rustc runs, in `libmimalloc-sys`' build script looking for
  `aarch64-linux-gnu-gcc`. `M0.md` now records the measurement beside the
  prediction, and the gate narrows to `--workspace --exclude oqueue` for a
  non-host target only when no cross compiler is present, saying so in its
  output. Where one exists it checks everything.

Review then found two more, both silent passes in the gate itself and both
fixed here: a `pub trait` inside an inline `pub mod` was invisible to the
fake-per-trait scanner because the pattern was anchored to column 0 — one level
of indentation bypassing the **only** automated enforcement of `contracts.md`
rule 9, since `check-core-contract.sh` holds no fake logic — and the
"is this gate invoked" check was a substring search, which `gates.yml` defeats
by naming three of the four gates **only inside comments**. Deleting a hook
block would have left a gate running nowhere while this reported it invoked.
⚠️ Both are the same shape the gate exists to catch, in the gate, on its first
day. A second round found the *same* mention-versus-invocation error surviving
in the CI half of that check — a workflow step named `- name: check-coverage.sh
(temporarily disabled)` counted as an invocation — and a negative case whose
expected substring named a command line that only appears when one of the two
hardcoded triples is the host, so the case would have gone red on macOS for a
defect that does not exist.

⚠️ **What the gate still cannot see, recorded rather than fixed**: a fake behind
`#[cfg(test)]` satisfies the fake-per-trait scanner, and no downstream crate can
name such a fake — which is the purpose `contracts.md` rule 9 states. Telling
the two apart needs real scope tracking rather than a line pattern, and so does
the related case: two same-named `pub trait`s in different modules are now
*counted* separately rather than collapsing, but either can be satisfied by the
other's fake, because matching is by name and not by resolved path. This is the
gate's own limit, and it belongs in M0's boundary review with the three parser
findings above it.

A fourth round found the worst one: **with `cargo` off `PATH` the whole gate
exited 0.** `require_tool` skips a missing tool, `finish` with no failures is a
pass, and every section below the cargo check needs no cargo at all — so a
minimal `PATH`, which is any container or non-login shell, made M0's own
completion gate declare the milestone complete having compiled nothing and
skipped the boundary-review refusal it exists to produce. `lib.sh` had already
made this exact departure for `require_python`, with a comment saying why.

⚠️ **What the gate still cannot see, beyond the `#[cfg(test)]` fake**: a `Fake`
struct and its `impl` written inside a `/** */` doc block satisfy the scanner.
The docstring first claimed block comments could only cause a false *red*; that
was true of traits and false of fakes. Solving it needs a block-and-string-aware
stripper, and the one `check-core-contract.sh` carries records two ways an
earlier version of it failed — one of which silently deleted every trait after
an unmatched `/*` in a string literal. Copied badly, it is worse than the hole.

⚠️ **`m0-complete.sh` fails today, and correctly**: `check-milestone-review.sh`
reports 13 of 21 M0 commits unread as a whole. That is the boundary review, and
it is the last thing M0 needs — the gate is doing exactly what it was written
to do by refusing to call the milestone complete before it happens.

**M0.23** did both halves, because either alone leaves the hole. The regex
gained `_ms` and `_seconds`; and ⚠️ **a regex is still a name matcher**, so
`m0-complete.sh` now asserts that every constant it pins is *matched by
`check-drift.sh`'s own `THRESHOLD_RE`* — read out of that script, not copied.
Measured with the old regex restored: `check-drift.sh` reports
`ok no threshold reads from the environment (60 file(s) scanned)` while the
gate reports `BUDGET_MS is invisible to check-drift.sh` and the same for
`COMPILING_GATE_MS`. That pair of outputs is the whole finding.

⚠️ `COMPILING_GATE_MS` is now pinned too, though no requirement names it: it
decides whether NFR-56's budget is *applied at all*, and a threshold that
switches another threshold off is the one most worth pinning.

⚠️ **What this does not fix**: the registry in `m0-complete.sh` is a list
somebody must add to. A constant that is in neither the regex nor that list is
still invisible, and nothing catches it — the gate closes the loop for
constants a requirement names, not for every constant. That is why `M0.16`'s
two got through, and it is the honest bound on this row.

**M0.24** put the claim under a gate, because `M0.1` had already fixed these
two files by hand and they were false again eighteen commits later — while the
tasks writing the very scripts the README called unwritten were landing.

⚠️ **The gate's first run caught the gate.** A single population for both files
reported `AGENTS.md` as false too, and that sentence is **true**: it is about
scripts the *standards* name, and `fuzz.sh` and `check-secrets.sh` are named by
`security.md` and written by nobody. One population would have forced a true
sentence to be deleted, which is the opposite of the point. Each claim is now
judged against the set it refers to — the README's against scripts skills
invoke, `AGENTS.md`'s against scripts standards name.

⚠️ **The assertion is a conditional, not a word ban.** "Some scripts are
missing" is a fine sentence on a day it is true. What cannot stand is writing
it on a day every script it refers to is present, and that disagreement is the
only form of this a script can judge.

Review then found three more, all in this row's own work. The population was
built from `scripts/<name>.sh` paths only, and `security.md` writes
`check-secrets.sh` bare — so the moment `M2` writes `fuzz.sh`, `AGENTS.md`'s
population would have been entirely present and the gate would have demanded
the deletion of a sentence still true. ⚠️ **A check whose failure mode is
"delete the true sentence" is worse than no check**; both futures are now
simulated, with `fuzz.sh` alone leaving it green and both scripts making it
fire. The README also claimed the sentence was "checked" when only the
conditional was — moving a skill-named script aside left the claim false and
the gate green, so the positive is asserted too: a *standard* may name a script
nobody wrote, but a **skill** naming one cannot run. And both files had grown a
hardcoded list of the two deferred scripts, nine lines below `AGENTS.md`'s own
warning that a hardcoded list of what is outstanding went stale four times
before the fix turned out to be deleting it.

⚠️ **And the gate's fourth property made two existing negative cases vacuous** —
their fixtures name no script, so they tripped the new inspected-nothing guard
as a second problem, and deleting the vendor-syntax check outright left the
suite green. Both now carry an `expect`. That is the same regression `M0.21`
found and the same remedy; a gate growing a property is the moment its cases
need pinning, and noticing it needed a reviewer both times.

**M0.25** corrected ten sentences and gated exactly one of them, which is the
distinction the row asked for rather than fixing ten and implying they stay
fixed. ⚠️ **Nine of the ten are prose against prose** — a rustdoc against an
ADR, a standard against the ADR that corrected it, a research document against
itself — and no script can hold those. The tenth is a **number**, so it got a
gate: NFR-56 is "2.27 s across N hooks", N is part of the claim, and three
documents held three different values.

⚠️ **The gate caught its author on its first run.** The count was written as 17
against a configuration of 18. That is the argument for the row in one line: a
number stated in prose is wrong the moment somebody adds a hook, including the
person adding the gate.

⚠️ **And the first version of the sentence announcing the gate was itself an
overclaim** — "so it cannot go stale again", written about a gate that runs at
a milestone boundary and is in neither the hooks nor CI, while covering one of
the two documents that held the stale number. Both are covered now, and the
sentence says what the gate is: boundary-only, and inherited by whatever
succeeds `m0-complete.sh` or not at all. ⚠️ Writing an overclaim into the
correction of ten overclaims is the strongest evidence this row could have
produced for its own premise. ⚠️ **And the number itself was wrong**: 18 is
every `- id:` line, but NFR-56's series counts the suite `check-budget.sh`
sums, and two hooks are `stages: [commit-msg]` — a separate invocation whose
rows can never enter that process group. The answer is **16**, and the gate now
parses the config with PyYAML and filters on stage rather than grepping, which
also closed a second hole: `^      - id: ` is exactly-six-spaces and blind to a
repo block written in pre-commit's own 4-space style. Three wrong versions of
one number, in the row whose whole subject is numbers stated in prose — and
then a **fourth site**: `check-budget.sh`'s own header still held a count,
twenty-four lines above a new comment saying no count was written there. It
now names where the live number lives instead of holding one.

⚠️ **And the parse must not need a third-party module.** `yaml` would have been
the only non-stdlib import in `scripts/`; "pre-commit depends on PyYAML" is
true of pre-commit's venv, not of system `python3` under a pipx or brew
install. Measured with the module shadowed: the gate aborted at 13 checks with
a message reading like a malformed config, and sections 6 through 8 — every M0
gate invoked and watched to fail, the negative suite, the boundary review —
never ran. It now falls back to a line parse that is indentation-agnostic and
stage-aware, and both paths return 16.

⚠️ The `[ADR-000n]:` reference-link definitions are rewritten as prose rather
than repaired as links. A link definition whose target is a code span resolves
to nothing and rustdoc renders the literal text — recorded as an `M0.9` minor,
then copied into two more files, which is the shape worth naming: a defect
recorded and not fixed is a defect that spreads.

**M0.26** made "a case that did not run" a thing a caller can *read*. The two
tool-gated cases were already skipped correctly — a missing tool is a skip,
never a failure — but the fact left no machine-readable trace, so
`m0-complete.sh` printed "every gate fails on a broken artifact" from the
suite's exit code and, on a machine without `cargo-llvm-cov` and
`cargo-mutants`, declared half of M0's four new gates complete having watched
neither fail.

⚠️ **An absent line fails too.** "Nothing skipped" and "this suite is too old to
say" are different states, and only one of them is a pass; before this they
were the same output. Verified both ways: hiding `cargo mutants` makes the gate
name `check-mutants.sh` as never watched, and deleting the suite's report line
makes it say the suite cannot answer.

⚠️ **And the row's own fixture walked into the defect review had named on
`M0.25`**: a missing `.pre-commit-config.yaml` aborted the gate at section 5b,
so sections 6 through 8 never ran — including the assertion this row adds. A
gate holding eight independent assertions has no business stopping at the first
unreadable file, and it no longer does.

**M0.27** is the first commit in this repository written from harvested commit
bodies. ⚠️ **That is not the procedure finding `bcf5d6f697f2` defers to M2** —
doing the collection once by hand decides nothing about who does it next time,
and the reason it was possible at all is that every minor had been written down
with a reproduction. The bodies were a *usable* store; what they are not is one
anything reads on its own.

⚠️ **Most of this row cannot have a negative case, and the reason is structural.**
`tests/gates/negative.sh` passes a case when the gate *fails*. Four of the six
fixes here remove a **false positive** — `_msg` no longer trips
`check-drift.sh`, a reflowed line no longer fails the hook count, `readonly
THRESHOLD_RE` no longer aborts the gate, a block-sequence `stages:` no longer
makes the two parse paths disagree. Each makes a gate *stop* failing on a good
artifact, which an inverted suite has no way to express. The measurements are
in the commit body and the real tree passing is the test; only the
`stages: [manual]` fix adds a refusal, and that one has a case.

⚠️ **Review then wrote two of the four cases the row said could not exist.**
The argument confused two things: a fix that makes a gate *stop* failing on a
good artifact is genuinely inexpressible here, but a fix that turns a **silent
abort into a reported failure** is not — the report is a string absent before
and present after, which is exactly what `run_case`'s fourth argument tests.
Both now have cases, and both were mutant-verified against the pre-fix form.
"It cannot be tested" is worth one attempt at disproof before it is written
down.

⚠️ **And narrowing a regex to fix a false positive opened a false negative.**
Anchoring `_ms` to a word end dropped `_msec` and `_msecs`, which are unit
spellings too, so `POLL_MSEC="${OQUEUE_POLL_MSEC:-500}"` — caught before —
passed a non-negotiable-2 gate after. Measured by review. Six spellings are now
checked in both directions.

⚠️ **Catching every exception went one step too far.** A config PyYAML cannot
load fell through to the regex fallback, which counted it happily, so the gate
reported a hook count and "every gate is invoked" for a file `pre-commit`
itself cannot read. The fallback is for a missing *module*; a broken document
is a `fail`.

⚠️ **And the case proving that was itself PyYAML-dependent.** Registered
unconditionally, on a machine without the module it reached the fallback,
printed no failure, and the suite reported that the gate had stopped catching a
defect — a misdiagnosing remedy, one file over from the row removing them. It
is `skip_case`-gated now, and a *second* case shadows PyYAML deliberately so
the fallback is executed by something: nothing else in the suite ever took that
branch, so the whole fallback — including the block-sequence handling this row
added — was dead test surface that could be deleted with everything green.

⚠️ **And review disproved the untestable claim a third time.** Narrowing a
regex has two halves, and only one is inexpressible here: `_msg` no longer
tripping the gate cannot be tested by a suite whose cases pass when a gate
fails, but `_msec` being caught *again* is a catch, and a catch is exactly what
this suite tests. One of six fixes is genuinely untestable now, not four. The
rule the file states — an untestable claim is worth one attempt at disproof —
was written in this row and then failed by it three times.

**M0.28** closes a gap that had been *recorded and re-recorded*: `M0.24`'s body
measured that three of check 3's four sub-properties deleted cleanly with the
whole suite green, named `M0.26` as their home, and `M0.26` did not do it. Each
now has a case, and each was mutant-verified — delete the positive check, merge
the two populations, or drop the inspected-nothing guard, and exactly one case
turns red.

⚠️ **The merged-population case is the interesting one.** Merging is not
obviously wrong to read: it only fails when a *skills* claim is false while a
*standards* deferral is legitimately outstanding, because the merged set then
contains a missing script and both halves skip. ⚠️ **Today's tree has the
second half and not the first** — two standard-named scripts are deferred, and
the README carries no such claim, so `check-portability.sh` is green at HEAD.
The fixture supplies the missing half, which is why the split needed a case
rather than a comment: the shape is one commit away and invisible until then.

`skip_case` now refuses an argument that is not a script name this suite knows.
⚠️ The contract between the suite and `m0-complete.sh` was a bare string matched
by word splitting, so a descriptive label — the convention every `run_case`
line uses — or a rename on one side only would have left a gate unproven while
the completion gate reported otherwise. That is `M0.26`'s own shape, one
argument over. It also reports `SKIPPED_COUNT` beside the names, because
`M0.26` was asked for a count and delivered gates: without `cargo-mutants` two
cases do not run and one `skip_case` fires, so the two numbers genuinely differ.

Review then found a **fourth** deletion path still green — dropping
`AGENTS.md`'s entry from `populations` left every case passing while the file
check 3's header names went unread. The merged-population case pins that the
two sets are *separate*; it does not pin that both exist. Two populations
needed two cases.

⚠️ **And two defects in this row's own additions.** `skip_case`'s rejection
branch ended in `return 1`, which under `lib.sh`'s `-e` aborted the suite there
— so neither `SKIPPED_COUNT` nor `SKIPPED_CASES` printed and `m0-complete.sh`
reported a *stale suite* when the cause was a typo'd argument. A misdiagnosing
remedy, inside the function added to stop one. And the new fourth positional
was undocumented while the signature comment above it still listed three, so
passing the count where the remedy goes printed a bare `2` and silently
undercounted.

⚠️ **What is still not pinned**: the four claim phrasings are a list, and a
sentence saying scripts are missing in some fifth wording passes. That is a
bound a regex cannot remove, and the docstring now says so instead of implying
timing was the only limit.

**M0.29** harvests the documentation minors M0 recorded. ⚠️ **It does not claim
to have closed the bound**, and the first four versions of this note did — each
time review found more inside it, and each correction of the claim was itself
false. The honest statement is: the items enumerated below are fixed, several
more were found and fixed during review, and **the `Recorded, not fixed`
sections of M0's commit bodies are not certified empty**.

⚠️ **That is the finding `bcf5d6f697f2` restated as evidence.** Harvesting by
hand does not terminate reliably — five rounds on this row found in-bound items
each time — which is exactly the argument for a *procedure* that collects them
at a boundary, and exactly why choosing one was deferred to M2 rather than
improvised here. Whoever opens M1 should read M0's commit bodies as an
unexhausted store, not as a closed one.

Eleven claims, each fixed or given a reason it stays:

- `review.md`'s "four lines below" was fourteen, and had been replicated into
  `roadmap.md` and `M2.md` — ⚠️ the same spread-a-known-defect shape `M0.25`
  named for the `[ADR-000n]:` definitions, in the commit that named it.
  `review.md` now says "below" and its two copies "just below" — no wording a
  later insertion falsifies.
- "31 minors" against "roughly thirty" in two other files: a number nothing
  derives and nothing acts on, replaced by "upwards of thirty" in all three.
- A doubled word in `roadmap.md`'s new row.
- ⚠️ Rule 15's interim instruction is now in both skills that load at review
  time. `M0.19`'s acceptance required those three to agree, and the sentence
  was inert exactly where it applied — it is the workaround for the deferred
  decision, so its absence *was* that finding recurring.
- `AGENTS.md` explained a missing script by an M-1 row `M0.16` closed. The
  operational instruction below it was right; the cause was stale.
- `.agents/skills/README.md` said seven standards where there are fourteen.
- `backlog.md` quoted `THRESHOLD_RE` in the present tense with a value two
  rows had since widened; it is now dated, and points at the script.
- `build.md` rule 7's "gates all four" is now marked as a property of today's
  manifest rather than of the gate — `overflow-checks = false` in
  `[profile.dist]` still ships a wrapping binary with the gate green.
- `testing.md` still called the full run sharded. ⚠️ Kept as the **target**,
  with a note that `M0.17` implemented the first half exactly and the second
  approximately — deleting the word would erase the gap.
- The three seam rustdocs rendered a literal `[ADR-000n]`; verified with
  `cargo doc` that none remains.
- `bin/oqueue`'s manifest named `MALLOC_CONF` — the unprefixed spelling read by
  nothing — in the feature comment whose subject is that
  `tikv-jemallocator/profiling` is the half that makes `heap-profiling`
  actually profile.

Review found items **inside the bound** that had been given neither a fix nor a
reason — a harvest declaring itself closed over ground it
had not covered. ⚠️ It arrived again in each of the next two rounds — five
items, then five more — which is why the note above no longer claims closure.
Closed here: `mutants.sh`'s missing `--timeout` against `testing.md` rule 18, doc 18's
section number in `oqueue-checksum`'s two documents, and two miscounts in the
notes themselves. ⚠️ **The timeout is a recorded decision, not an
implementation**: `cargo-mutants` derives its cap from a baseline run under the
same parallelism, which is what rule 18 asks for, and a literal would be a
threshold nobody has measured — so the rule now names the script and the script
argues the default.

Also closed, each recorded in an M0 commit body and inside the bound: `M8.md`'s
deliverable 2, whose surviving text put the `KeyProvider` fake in
`oqueue-testkit` — ⚠️ **nothing checks a milestone plan's file locations**, so
an M8 executor building from it would have done what ADR-0006 and
`contracts.md` rule 9 reject; `oqueue-broker`'s README invariant row, which said
the real `Clock` lives there against its own note two lines down, in the one
crate `check-sans-io.sh` exempts from the clock pattern; and the three leaf
crates' `lib.rs`, which folded the differential-property-test obligation into
the `check-unsafe.sh` sentence and so made a review-held rule read as gated.

Then, in the round after that: `.pre-commit-config.yaml`'s orphaned comment
fragment and its two ordering claims (one said three hooks were "scrolled
past"; both argued from output length while reading as speed), `clock.rs`'s
"every `expect` is on a generated value" where four of five are literals,
`backlog.md`'s claim that ageing excludes an abandoned run's rows when the sum
runs before the prune, and ADR-0002's FR-50 citation where FR-31 is the
requirement plus its "it compiles" measured on the desugared form and not on
the shape that does not compile at all.

⚠️ **What is left open, with a reason**: `0ef46cb4a5a4` — that no backlog row
exists for a `reviews/`-only terminating commit to name — stands, because
`M0.22`'s body argues the answer (reuse an existing row, per `M-1.37`) and
writing that convention into `git.md` is a process change rather than a
documentation correction.

⚠️ **What is deliberately not fixed**: the `AGENTS.md` files whose "Easy to get
wrong here" sections duplicate their README's notes, and the four copies of the
`Waker::noop` busy-poll `block_on`. Both are M0.22's minors and both are M1's —
M1's runtime deletes the second outright, and the first is a rewrite with no
gate to hold it.

⚠️ **This note deliberately states no count.** Three attempts were made — the
finding's "every crate", then eight, then nine — and review measured each of
them false; the third was produced by a script whose bullet pattern was too
narrow to see half the matches. A number nobody can re-derive on demand is
exactly what `M0.14`'s census exists to avoid, and prose is the wrong place for
one.

⚠️ **What is true and load-bearing for the M1 rewrite**: most of the twelve
duplicate their README's notes wholesale, and **`oqueue-core` and `bin/oqueue`
do not** — `bin/oqueue`'s carry the allocator choice, the aarch64 link gap and
the never-executed heap-profiling build. ⚠️ Only the last of those three is
unique to that file; the other two are also in its README. An executor scoping
a rewrite from "every crate" would still delete a warning with no other home. The
count is `diff <(...) <(...)` away for whoever does the work, and belongs in
that commit rather than here.

**M0.30** closes a gap that only a boundary review could see: every gate in
`scripts/` was built against the rule *"a gate nobody has watched fail is a
gate nobody has tested"*, and that rule was **in no standard** — `grep -rin "watched to fail"
docs/internal/standards/` returned nothing. It lived in backlog acceptance rows
and one script header. It is `testing.md` rule 20a now.

⚠️ **And the suite enforcing it was about to stop running.**
`tests/gates/negative.sh` and `check-milestone-review.sh` were invoked only by
`m-1-complete.sh` and `m0-complete.sh` — scripts nothing invokes, which the
`milestone` skill runs only while their own milestone is current. `M0`'s
completion gate says so in its own header and then fixes it for one milestone;
`M-1`'s did the same. The same repair, applied twice by hand, generalised
never. CI now runs the negative suite per push. ⚠️ **Only that one** — see the
paragraph below on why `check-milestone-review.sh` stays out, and note that
`reviews/README.md`'s claim about CI is therefore still a claim about
capability rather than about a wired step.

⚠️ **Not a pre-commit hook**, and that is the constraint that shaped it: the
suite builds several cargo fixtures and NFR-56 gives the whole hook suite 10 s.
Per-push is where it fits, which means a commit can still be made locally with
a gate nobody has watched fail — the milestone completion gate remains the
backstop for that.

⚠️ **And `check-milestone-review.sh` is *not* in CI**, which the first version
of this row got wrong. It is a **completion** gate — `review.md` rule 11 says
it refuses a milestone's *completion*, and its own header permits commits to
accumulate between reviews — so a per-push step is red for most of every
milestone's life, on a tree where every other gate passes. Review measured it:
red at 4 of 31 the moment it was wired. A gate that is red by design most of
the time makes a real failure indistinguishable from work in progress, which is
a finding `gates.yml`'s own header already carries. ⚠️ `reviews/README.md`'s claim
that "a second agent, a fresh clone, and **CI**" can all check milestone
coverage is a claim about *capability* — the verdicts are tracked, so CI could
— and it stays that, rather than being made into a wired step that would cry
wolf. Making it true needs a gate that knows what a *closed*
milestone is, which nothing does today.

**M0.31** is two documents asserting the opposite of what is true, worth
keeping together because they fail in mirror image. ⚠️ **Both came from
per-commit review, not from a boundary-review artifact** — `grep socket
reviews/milestone-M0-*.json` finds nothing — which is the same store
`bcf5d6f697f2` is about, and the reason this is a row rather than a sentence in
a commit body.

`oqueue-broker`'s Invariants row said the crate "names no concrete backend,
clock or **socket** type" — in the same file, below its own "the sockets have
to be somewhere. This crate is that somewhere", and beside the `check-sans-io.sh`
exemption that exists to permit them. ⚠️ **An invariant that the first commit
writing the connection loop must falsify**, in a row whose own cell says review
is the only thing holding it. The row is now about seam implementations, and
says explicitly that sockets are excluded and why. ⚠️ **`Clock` came out of the
row too** — `M0.29`'s separately recorded, unscheduled minor, closed here:
`check-sans-io.sh`'s exemption says the real one lives in this crate,
`architecture.md` says `bin/oqueue` chooses the concrete types, and ADR-0004
calls it "`M1`'s real `Clock`". Asserting either side re-creates the socket
defect, so the README recorded what looked like a disagreement and left it to
M1, with whichever side loses to be corrected in the same commit. ⚠️ **`M1.29`
found there was no disagreement** — the three documents answer different
questions — so nothing lost and nothing needed correcting on those grounds;
what did need correcting was this framing, in all three places `M0.31` put it.

`M1.md` routed "ADR-0005's FR-50 citation where FR-31 is the requirement" —
and the **prescription** is wrong: ADR-0005 disclaims FR-31 deliberately, in a
paragraph of its own, so an executor following that bullet would have edited a
correct disclaimer into a contradiction. ⚠️ **The citation is still a defect**,
and saying it was "correct" — which this row's first attempt did — replaced one
false claim about a named artifact with another pointed the other way. It is one of
the five survivors `M0.29`'s body **counts without naming**; `M1.md` enumerates
them, and the fix is to drop the parenthesis or cite NFR-51, the ADR's own
`Requirements:` header. The lesson is
`review.md` rule 12a one level up: a claim about a named artifact, written
without opening it, by the commit whose subject was routing accurately-described
defects.

**M0.12** found that three standards had an `applies_to` glob that never
matched. `contracts.md`, `behavior.md` and `async-concurrency.md` all listed
`"*/bin/oqueue/*"`, which cannot match a repository-relative `bin/oqueue/...`
path — `which-standards.sh` returned six standards for the composition root
where it should return nine. ⚠️ **The first review of `bin/oqueue` was therefore
handed a packet missing the three standards that name it explicitly**, including
`contracts.md`, and the reviewer found the bug rather than the packet. Fixed by
adding the anchored form alongside the existing one; `M0.13` touches the same
directory and would have hit it again.

**M0.3** found a factual error in doc 18 §3.7.2, which the ADR now corrects.
"Five profiles are five independent target trees" is **four**: `--profile dist`
writes `target/dist` and `--profile release-checked` writes
`target/release-checked`, but cargo gives the built-in `bench` profile **no
directory of its own** — it writes `target/release`, the same path
`--profile release` writes. So a benchmark run replaces `target/release/` with a
fat-LTO, full-debuginfo binary that is not the release build, and anything later
packaging "whatever is in `target/release`" ships it — silently, since both
profiles' objects coexist in `target/release/deps` under different metadata
hashes, so alternating rebuilds nothing and only the uplifted artifact changes.
⚠️ The first correction claimed the two *evict* each other and that every
alternation is a full rebuild. Review disproved it by measurement — 0.01 s each
way — which is the second time in this task a sentence about cargo was written
from plausible reasoning and had to be replaced by an observation. Nothing in a profile table can
express the fix, so ADR-0001 states it as a rule on the harness instead —
anything that benchmarks sets its own `CARGO_TARGET_DIR` — which binds `M13` and
`M14`. Found by review, and reproduced before it was believed: the first draft
of the ADR had repeated the doc's sentence and told the reader to delete a
directory that does not exist.

⚠️ **And the rule bound something that already existed**, which the first fix
missed by writing it as a constraint on `M13` and `M14` alone. `scripts/bench.sh`
and `scripts/profile.sh` were written in M-1 and already run `cargo bench`; a
second review round found them. They set `CARGO_TARGET_DIR` as of this commit —
correctly, because **this** is the commit that makes `bench` diverge from
`release`, so before it the collision was two names for the same codegen and
after it the collision ships the wrong binary. Written as a *subdirectory of*
any outer `CARGO_TARGET_DIR` rather than a fallback for one: a fallback would be
defeated by the CI cache or worktree setting that makes an override likely in
the first place. Doc 18's two wrong sentences — §3.7.2's "five independent
trees" and §3.7.3 item 6's "delete `dist` and `bench`" — are corrected in place
with a pointer to the ADR, on the same reasoning M0.2 used for `README.md`'s
status banner: the commit that falsifies a sentence is the commit that fixes it.

⚠️ **Four of the five profiles are built by nothing, and that is a finding for
M0's boundary review** rather than something `M0.3` closed. `check-crate.sh`,
the hooks and CI all compile `dev` and `test`; `M0.3` checked all five on both
targets **by hand, once**. A later change that breaks `--profile dist` — a
dependency that fails under fat LTO, an overflow that only `overflow-checks`
catches — surfaces at `M13` when someone tries to cut an artifact. Unlike the
rest of ADR-0001's unguarded list this one is closable, by a `cargo check
--profile dist` step in CI, and it is recorded rather than done because adding a
CI step is not what this row asked for and `M0` should not decide by omission
that four profiles go unbuilt.

⚠️ One item from that list was **scheduled rather than escalated**, and the
distinction is the point: `security.md` rule 4 does not merely leave
`overflow-checks = true` unguarded, it **names** the gate — "→ gate on the
profile setting" — and none exists. That is the class M0.1 found twice over in
`security.md` and could only record. This one has a home: `M0.21` gains a
manifest assertion in `check-layering.sh` for `lints.workspace = true` and for
member-crate `[profile]` sections, so the root manifest's `overflow-checks`
lands beside them, in the same script, under the same obligation to add a
negative-test case. The window it leaves open now runs to `M0.21`, and no task
in it touches a profile. Found by review, which was right that an ADR listing it
next to three genuinely ungateable properties would have read as a fourth one.

**M0.4 through M0.6, and M0.19**, recorded their learnings in commit bodies and
not here — and the checkpoint review named that as the mechanism by which two of
its own major findings went missing. What belongs in this file rather than in a
message:

- **ADR-0002 chose one `dyn`-compatible trait per async seam**, not `async fn`
  in trait. The first draft specified a bridged pair, and as first written it
  did not compose — `Arc<dyn DynObjectStore>` is not an `ObjectStore`, so the
  composition root could not feed a generic consumer (`E0277`). ⚠️ **The full
  form does compile**, with both input lifetimes tied and an explicit
  `impl ObjectStore for Arc<dyn DynObjectStore>`; ADR-0002 records that as
  measured and rejects the pair on cost, not on feasibility. `contracts.md`
  rule 7 already made the
  `dyn`-compatible trait the default and said the exception needs a reason; the
  reason drafted was NFR-2's cached-read budget, which is verified by **zero
  object-storage round trips**, so an `ObjectStore` call is not in it.
- **No M0 task takes an async dependency**, and `M0.10`'s row now carries how:
  `Waker::noop()` and a busy poll. `M0.16`'s NFR-56 floor rests on this, and
  `check-layering.sh` does not read `[dev-dependencies]`, so nothing would catch
  a violation.
- **A containment check is not a redaction test.** `M0.6` screened error output
  for the ASCII secret and for `{:?}` of the byte vector; rendering the same
  bytes as `{:x?}` leaked all 32 with the suite green. Pinning each rendering
  with `assert_eq!` closed it. ⚠️ What actually makes a leak impossible is the
  *missing* `T: Debug` bound in `redacted.rs` — review confirmed every leaking
  mutation had to add that bound before it would compile. The tests are the
  regression guard on the property, not the property.
- **`M0.19` shipped two rules of the five it set out to write.** Comment-density, criterion-length and
  ADR-length rules were dropped: no gate, and the criterion-length one made this
  very cap binding in the commit that filled slot 20. Writing less is a
  behaviour; a document telling yourself to write less is more writing.

## M-1: AI development system

Build the system that builds everything else. Ordered by dependency — the
product docs come first because everything else references them, and `lib.sh`
comes before any gate because every gate sources it.

| ID | Task | Acceptance | State |
| --- | --- | --- | --- |
| M-1.0 | Repo foundation: `git init`, Apache-2.0 `LICENSE`, `.gitignore`, `rust-toolchain.toml`, and the research corpus placed under version control | Files exist; toolchain pins 1.97.1 and both Linux targets | done |
| M-1.1 | `AGENTS.md` + `CLAUDE.md` adapter, non-negotiables with honest enforcement marking | Every rule names its gate task or is marked unenforceable; no rule claims a gate that does not exist, and no link points at a file that is not there | done |
| M-1.2 | `mission.md` — what this is, what it costs, what it must never do | States the latency position and the scale target with citations into the corpus | done |
| M-1.3 | `architecture.md` — crate map, dependency rule, the seams | Names all eleven crates, the star rule with its two exceptions, and the `unsafe` budget | done |
| M-1.4 | `roadmap.md` + `backlog.md` — milestones with executable completion conditions | Every milestone's "done" is a command; only M-1 is decomposed | done |
| M-1.5 | `standards/` — remaining code standards: `behavior`, `rust-style`, `error-handling`, `async-concurrency`, `contracts` | ⚠️ Every carried-over threshold is re-derived against oqueue or explicitly marked underived | done |
| M-1.6 | `scripts/lib.sh` + `check-commit-msg.sh` | Commit subject must name a task this file lists; `require_tool` skips with a named remedy rather than failing | done |
| M-1.7 | `check-drift.sh` + `check-tests-kept.sh` | A threshold made settable fails; a deleted test without `Removes-test:` fails | done |
| M-1.8 | `check-layering.sh` + `check-sans-io.sh` | Sideways dependency fails; a concrete socket type, real clock read, or object-store call in a library crate fails | done |
| M-1.9 | `review.sh` + `check-reviewed.sh` — the isolated reviewer and its gate | Review artifact keyed by staged-diff hash; amending one byte after review fails the commit | done |
| M-1.10 | `check-core-contract.sh` | A `pub trait` method-set change without every implementor and an ADR in the same commit fails | done |
| M-1.11 | `check-unsafe.sh` | `unsafe` outside the three named crates fails; every `SAFETY:` block has a baseline entry | done |
| M-1.12 | `check-budget.sh` — the pre-commit time budget as an enforced constant | Suite over budget fails; timings written as an artifact so erosion shows as a trend. ⚠️ **Blocked, and structurally so** — NFR-56's constant cannot be chosen without a workspace to measure, which is why `m-1-complete.sh` names no non-negotiable that depends on it and M-1 completed with this row open. **`M0.16` writes both the script and the number and closes this row**; it is not a task to pick up before then | done |
| M-1.13 | `.agents/skills/` — `milestone`, `next-task`, `spec`, `tdd`, `review`, `adr`, `research`; `.claude/` adapters and the isolated reviewer subagent | Each parses as the Agent Skills spec; each calls `scripts/`, never a tool built-in; no vendor syntax outside `CLAUDE.md`; adapters contain pointers, not procedures | done |
| M-1.14 | `.pre-commit-config.yaml` (direct-to-main) + push-triggered CI | ⚠️ No gate keyed to `origin/main...`; PR-triggered gates rebased onto the previous commit | done |
| M-1.15 | `tests/gates/negative.sh` — prove every gate can fail | Each gate invoked against a broken artefact and observed to fail | done |
| M-1.16 | `scripts/gates/m-1-complete.sh` — the milestone's own completion condition | Asserts every non-negotiable names a passing script, except rule 3; calls `check-milestone-review.sh`, so the milestone cannot be completed while any of its commits has gone unread as a whole | done |
| M-1.17 | Make the corpus and product docs self-contained before the repo goes public | No reference to any other repository, no absolute local path, no verbatim quotation of an external private source; every practice stated as this project's own standard | done |
| M-1.18 | Public-facing files: `README.md`, `CONTRIBUTING.md`, `SECURITY.md` | README states plainly that no implementation exists; contributing says code is not yet accepted and why; security gives a private reporting route | done |
| M-1.19 | Record the BYOK and FIPS requirements across mission, architecture, roadmap, and the corpus | New milestone M8; `KeyProvider` seam and `oqueue-crypto` crate in the architecture; the AEAD-algorithm-in-region-header constraint recorded against M1 | done |
| M-1.20 | Revise the encryption design for the clarified BYOK volume (~10K topics, not catalog-wide) | Segregation replaces universal per-region sealing; the KMS key-count conclusion corrected and marked as corrected | done |
| M-1.21 | `requirements.md` — functional and non-functional, with stable IDs | Every entry names a verification; every NFR carries a number or is marked UNDERIVED with what blocks it; nothing invented | done |
| M-1.22 | `standards/sdd.md` — the process standard | Defines the requirement→spec→task→commit chain, what a spec must contain, acceptance-criteria rules, definition of done, and what to do when a spec proves wrong | done |
| M-1.23 | `standards/review.md` — the operational review standard | Turns [docs/researches/21](../../researches/21-ai-development-loop.md) §3–5 into a standard: reviewer context isolation, the deterministic/semantic split, fixed-or-argued resolution | done |
| M-1.24 | Trace every milestone to the requirements it serves | Every roadmap entry names FR/NFR IDs; a gate fails on a milestone that names none | done |
| M-1.25 | `standards/security.md`, `performance.md`, `build.md`, `portability.md` | Each rule names its gate or is explicitly marked as having none; rationale delegated to the corpus rather than restated | done |
| M-1.26 | `standards/code-structure.md` + `standards/testing.md` + `clippy.toml` | File ≤500 lines with a reasoned allowlist; function ≤50 lines, cognitive complexity ≤20, ≤5 arguments, all via `clippy.toml`; per-crate `README.md` and `AGENTS.md` required; fakes over mocks; the no-flake rules | done |
| M-1.27 | `check-file-size.sh` + `check-readmes.sh` | File-size limit with an allowlist whose entries carry reasons; every crate has both documents, and the README's stated dependencies match `Cargo.toml` | done |
| M-1.28 | `scripts/profile.sh` + `scripts/bench.sh` | Every profiling mode is one command: instructions, flamegraph, heap, massif, cache, allocation counts. **Never gated** — available on demand | done |
| M-1.29 | `check-hot-path-bench.sh` | A benchmark's `hot-path:` marker names a real row in `performance.md` rule 18's table; a row with no marker fails unless it is named, with a reason, in the script's `NOT_YET_BUILT` allowlist — see rule 19 and the retrospective below for why | done |
| M-1.30 | `check-portability.sh` — tool portability of the agent system | `AGENTS.md` and every `SKILL.md` parse without vendor syntax; every skill has `name` and `description`; every `.claude/commands/*.md` is a pointer rather than a procedure | done |
| M-1.31 | `contract-change` skill | The atomic `oqueue-core` trait change: trait, every fake, every implementation, call sites, and the ADR in one commit | done |
| M-1.32 | `standards/git.md` — commit atomicity and structure | States why atomicity matters (bisect is the substitute for a reviewer), the subject and body rules, amend-before-push / follow-up-after, and the no-branching workflow | done |
| M-1.33 | Frontmatter on standards and product docs + `scripts/build-index.sh` | Every standard and product doc carries a `description` saying *when to read it*, so layer-1 disclosure works for them as it does for skills; generated index regions rebuild from frontmatter and `--check` fails a stale one | done |
| M-1.34 | `applies_to:` frontmatter + `scripts/which-standards.sh` — route a change to the standards it is judged against | Every standard declares the paths it claims, and one without `applies_to` fails; a staged diff resolves to standards without anyone choosing | done |
| M-1.35 | `check-commit-msg.sh` dies silently on an all-comments message | `grep -v '^#' \| head -1` under `pipefail` exits 1 with no output; the gate must name itself and the reason | done |
| M-1.36 | Rename the `goal` skill to `milestone` | No skill, adapter, or index entry is named `goal`; every reference resolves and `build-index.sh --check` passes | done |
| M-1.37 | The outer loop: `milestone-review` skill + `milestone-review.sh` + `check-milestone-review.sh` | Every commit in a milestone is covered by a review artifact naming the commits it read; a blocking or major finding must name a backlog task that exists, or be argued; an uncovered commit fails the gate | done |
| M-1.39 | `known_task_ids` reads the backlog from the working tree | Every other input to `check-reviewed.sh` and `check-milestone-review.sh` is read from the index; an unstaged backlog row satisfies a gate locally and fails the same gate on CI. Shared by three gates, so it is not M-1.37's to change | done |
| M-1.38 | `check-reviewed.sh` matches a task id as a regex | `grep -qx "$task_id"` against the backlog's ids: an artifact whose `task_id` is `.*` matches every row. `grep -qxF`. Found by M-1.37's review at the sibling site | done |
| M-1.40 | The plan layer: `milestones/` + the plan-vs-backlog distinction | `sdd.md` states the difference between a *plan* (forward-looking, expected to be re-derived) and the *backlog* (authoritative, current milestone only); `roadmap.md` carries every milestone to v1 with a kind, the requirements it serves, its dependencies, and an execution order that is not numeric order; `milestones/README.md` says how a plan is consumed | done |
| M-1.41 | Milestone plans: M0, M1, M2, M3, M10 | The build-out sequence — workspace, object store, protocol, coordinator, deterministic simulation. Each names its goal, kind, requirements, the ADRs that must be written before its code, ≤20 provisional tasks, and a completion condition that is a command | done |
| M-1.42 | Milestone plans: M9, M4, M11, M5, M6 | Security, consumer groups, idempotence, compaction, recovery. Same shape as M-1.41 | done |
| M-1.43 | Milestone plans: M7, M8, M12, M13, M14, M15 | Scale, encryption, admin, release engineering, performance validation, hardening. Same shape as M-1.41 | done |
| M-1.44 | `portability.md` — a shell-scripting section documenting the `set -o pipefail` SIGPIPE-under-`grep -q`/`head`/early-exit idiom | Names the idiom that recurred nine times across six scripts in this milestone (`build-index.sh`, `check-reviewed.sh`, `check-layering.sh`, `check-core-contract.sh`, `milestone-review.sh`, `check-milestone-review.sh`), gives the `\|\| rc=$?` / here-string remedies as rules, and is cited by name from at least one gate script comment rather than left as tribal knowledge in commit messages and backlog prose | done |
| M-1.45 | Backport the crash-safety wrapper (`try`/`except Exception` around the Python body, distinct exit code 3 for an uncaught crash) from `build-index.sh`/`check-unsafe.sh` to `check-layering.sh` and `check-core-contract.sh` | Both scripts were written after `fadd095` (M-1.33's follow-up) established the wrapper and after `check-unsafe.sh` reused it, but neither adopted it; both currently avoid a false *pass* on crash only because no `print` executes before their risky `git`/file-read calls — an undocumented, unenforced invariant one added diagnostic print away from silently reintroducing the exact false-pass class the wrapper exists to prevent. Acceptance: both scripts exit a distinct non-1/0 code on an uncaught exception, verified against a contrived non-UTF-8 input the way `check-unsafe.sh`'s own suite already does | done |
| M-1.46 | `tests/gates/negative.sh` — a permanent case for `scripts/gates/m-1-complete.sh` | A broken artifact (`AGENTS.md` missing its `## Non-negotiables` section, and non-UTF-8 `AGENTS.md` bytes to exercise the crash wrapper) makes `m-1-complete.sh` fail, checked in and re-run on demand instead of the five ad hoc scratch repos M-1.16's own commit message and backlog retrospective describe running once and not preserving — the exact standard M-1.15 established for every other gate one commit earlier | done |
| M-1.47 | Re-sync `milestones/M-1.md`'s "Notes for the boundary review" and `roadmap.md`'s M-1 task-count cell after a milestone-review checkpoint | `M-1.md` no longer says "no commit in M-1 has been read as a whole" / "the coverage is zero" once `reviews/` holds an artifact that says otherwise, and the roadmap's task-count cell for M-1 matches `backlog.md`'s actual row count — the `milestone-review` skill's "Then re-plan: amend the roadmap with what was learned" step, skipped after the M-1.37 checkpoint | done |
| M-1.48 | `check-commit-msg.sh` has the same pipe-form SIGPIPE misreport `check-reviewed.sh` and `check-milestone-review.sh` were fixed for | `printf '%s\n' "$known" \| grep -qx "$id"` at line 133 — a large enough backlog makes `grep -qx` exit at the first match, `printf`'s remaining write SIGPIPE, and `pipefail` report a real, listed task id as unlisted. A seventh site of the class `M-1.44` names; not in that task's own list of six. `grep -qxF "$id" <<< "$known"`, matching the sibling fix's shape. Found by M-1.38's review | done |
| M-1.49 | Re-sync `milestones/M-1.md`'s "Notes for the boundary review" after the third milestone-review checkpoint | The section's Checkpoint 2 bullet 3 no longer states, in the present tense, that `M-1.38` and its sibling `M-1.39` "are both live defects ... confirmed still present" — both are fixed (`scripts/check-reviewed.sh` now uses `grep -qxF ... <<<`, `scripts/lib.sh`'s `known_task_ids`/`open_task_ids` now read the index via `_backlog_from_index`) and both backlog rows are `done`; the section gains a Checkpoint 3 entry recording what this checkpoint found instead, the same shape checkpoints 1 and 2 used | done |
| M-1.50 | `check-hot-path-bench.sh` reports only the first failing category per run, not every violation in one pass | The script's three checks (unknown marker, stale `NOT_YET_BUILT` entry, uncovered required row) each end in an early `finish` on failure, so a commit with more than one kind of defect at once only sees the first — reproduced directly: a fixture with both an unknown marker and a `NOT_YET_BUILT` entry that has become stale reports only the unknown-marker failure, and the stale-entry failure only surfaces once that is fixed and the gate re-run. Contradicts `scripts/lib.sh`'s own `fail()` contract ("the caller keeps going so one run reports every violation rather than only the first"), which every sibling gate in this milestone (`check-requirements-trace.sh`, `check-file-size.sh`, `check-readmes.sh`, `check-portability.sh`) follows by accumulating all problems before a single terminal report. Acceptance: all three checks run unconditionally and every violation across all three is reported in one invocation | done |
| M-1.51 | `tests/gates/negative.sh` — a permanent crash-path case for `check-layering.sh` and `check-core-contract.sh` | `M-1.45`'s crash-safety wrapper was verified only in scratch fixtures, not checked in — no gate in this repository currently exercises the crash path for any script, including `check-unsafe.sh`, whose own suite the `M-1.45` acceptance criterion pointed at as precedent and which turns out not to have one either. `tests/gates/negative.sh` already carries `run_case`/`setup_*`/`invoke_*` scaffolding for both scripts a companion `(non-UTF-8 crash)` case can reuse directly, the same shape `check-reviewed.sh (regex task_id)` and `check-hot-path-bench.sh (required row)`/`(leftover entry)` already use for a second case against one gate. Found by `M-1.45`'s own review | done |
| M-1.52 | Re-sync `milestones/M-1.md`'s "Notes for the boundary review" and `roadmap.md`'s M-1 task-count cell after the milestone-review checkpoint covering M-1.31, M-1.44 through M-1.46, and M-1.48 through M-1.51 | `M-1.md`'s closing paragraph still names `M-1.31`, `M-1.44` through `M-1.46`, and `M-1.48` as open although `backlog.md` marks all of them `done` (and doesn't mention `M-1.49`–`M-1.51` at all); its "Tasks" table still lists `contract-change (M-1.31)` under "Remaining" though it landed; `roadmap.md`'s M-1 task-count cell says 48 against `backlog.md`'s actual 52 rows. `M-1.49` claimed to replace a hardcoded ID list with something that wouldn't need remembering to update and, read against its own diff, replaced a 2-ID list with a 5-ID list — the identical fragile shape, which is why it went stale again one batch later. Acceptance: add a Checkpoint 4 entry, correct the stale prose and table, fix the task-count cell, and replace both hardcoded task-ID lists with something derived from `backlog.md`'s actual open rows rather than a list an editor has to remember to update by hand. Found by the boundary milestone review | done |
| M-1.53 | `tests/gates/negative.sh` — a case for `check-unsafe.sh`'s handling of unreadable input | `check-unsafe.sh`'s crash-safety wrapper has existed since `M-1.11` and had never been exercised by any test. The literal acceptance this row originally asked for — a `(non-UTF-8 crash)` case in the same shape `M-1.51` gave `check-layering.sh`/`check-core-contract.sh`, verified via a `CRASH ...` line — turned out not to be satisfiable: probed directly, a non-UTF-8 byte in a tracked file's content, its filename, or `baselines/unsafe.txt` are all already caught before reaching that wrapper, unlike the sibling scripts before `M-1.45`. See the retrospective for the investigation and the adjacent property implemented instead: `setup_unsafe_non_utf8`/`invoke_unsafe_non_utf8`, proving an unreadable file's `warn`-and-skip does not stop the scan before a real violation elsewhere, mutant-tested against the loop's own `continue`. Found by the boundary milestone review | done |

### Notes on specific tasks

**M-1.5** carries the hazard the hybrid decision named: a ported rule justified
by the other project's wire protocol, or a threshold derived from a test suite
that does not exist here, is **stale but authoritative** — worse than absent.

`behavior.md`'s scope was not specified anywhere before this task and had to
be decided rather than found: it is the *external* contract — what a Kafka
client, an operator, or another tenant may rely on staying true — as opposed
to the other four, which govern how the Rust reads and is structured
internally. That split is stated at the top of the file itself so a future
reader does not have to reverse-engineer it from the table of contents.
`code-structure.md` already covered the `unwrap`/`expect` ban and the
`unchecked_*` ban, and `testing.md` already covered where fakes live; the
four new files reference those rules rather than restating them, to avoid
the two places-one-fact hazard M-1.33 already named for generated indexes. Every gate a rule names is one of
M-1.7 through M-1.12, none of which exist yet — consistent with every other
standard in this repository, which routinely names a gate before the script
behind it is written.

**M-1.7** wires the second half of non-negotiable 2 — the first (never
lower a threshold) is `check-drift.sh`, forward-looking and heuristic,
since the repository has almost nothing for it to check yet: it flags a
threshold-shaped identifier read from an environment variable on the same
line, which is deliberately narrower than "any threshold ever weakened,"
a claim no grep gate can make honestly without a real parser. The second
(never delete a test to make a check pass) is `check-tests-kept.sh`, which
diffs `#[test]`-attributed function names between a file's old and new
content and demands a `Removes-test:` trailer for anything that
disappears — it does not and cannot judge whether the stated reason is
true, the same limit `check-commit-msg.sh` already accepts for task IDs.
Both were run against a contrived positive case in a scratch git repository
before being trusted, rather than only read — the same discipline M-1.6's
retrospective names ("a gate nobody has watched fail is a gate nobody has
tested"), ahead of M-1.15 making that a permanent, checked-in negative
suite rather than a one-off manual run.

⚠️ **That manual testing still missed the one check that mattered most: did
`check-drift.sh` pass on the repository it was being added to.** Review
found it did not — its own header comment states the two patterns the gate
looks for side by side, as worked examples, and both matched its own regex,
so the gate would have failed on itself forever from the moment this
commit landed. Fixed by excluding the script's own file from the scan, with
the reason written beside the exclusion rather than left implicit — it
holds no threshold of its own to protect, so nothing is lost, and a
scratch-file test with the same worked-example text under a different
filename confirmed the exclusion is narrow rather than a general carve-out.
Review also found a doc-comment claim in `check-tests-kept.sh` that was
backwards: it said a test moved to another file under the same name is
*not* flagged, when the per-file comparison the code actually performs
flags exactly that case, in the file the test disappeared from — the safer
direction to be wrong in, but the comment described the opposite of the
code next to it.

⚠️ **A second review round found the manual testing had checked the common
shape and no other.** `extract_tests`'s awk cleared its "pending test" flag
on any non-blank line that was not itself a `#[...]` attribute — so a
`#[test]` immediately followed by a doc comment, or by a multi-line
attribute's continuation lines, never reached a `fn` while the flag was
still set, and the test underneath was invisible to both the old and the
new side of the diff. A test deleted in that shape passed with `ok no test
was removed`: the exact acceptance criterion this task exists to satisfy,
silently unmet for two idiomatic Rust shapes. Fixed by letting the pending
flag survive doc comments, plain comments, blank lines, and an unmatched
multi-line attribute's continuation (tracked by an unbalanced `#[` with no
`]` yet) — verified against both shapes in a scratch repository, plus a
regression pass confirming the original cases (single-line attribute,
whole-file deletion, rename via delete-and-add) still hold.

⚠️ **A third review round found the second round's fix was still only the
common shape.** The continuation tracker set `pending` on close without
ever checking whether the attribute it had just spanned actually contained
"test" anywhere — only the *opening* line was checked, and a wrapped,
parameterized attribute like `#[tokio::test(\n  flavor =
"multi_thread"\n)]` puts `test` on that opening line by coincidence of
`tokio::test`'s name, while a differently-named or differently-wrapped
multi-line test attribute would not be recognized at all. Verified directly
with a `#[tokio::test(...)]`-shaped attribute deleted with no trailer: `ok
no test was removed`, silently. Realistic for this specific project, not a
contrived case — async test attributes wrapped across lines by `rustfmt`
are exactly the shape `async-concurrency.md`'s own subject matter produces.
Fixed by checking every line the attribute spans for "test", not only the
first, and setting `pending` when the attribute closes if any of them
matched. Verified against the exact wrapped-`tokio::test` case, a full
regression re-run, and the disclosed nested-`[...]`-in-a-string gap,
reconfirmed real. ⚠️ That reconfirmation needed a second try: the first
attempt at constructing it wrote a single-line `cfg_attr`, which the
single-line branch handles correctly regardless of the nested bracket and
so never exercised the continuation-tracker code path it was meant to
test — a version of the same lesson this task keeps producing, that a test
which looks like it covers a case can silently cover a different one.
Redone with a genuinely multi-line attribute, it reproduced.

A fourth review round, asked to try shapes no prior round had, found one
more: the `fn`-matching regex accepted `pub` and `pub async` but not
`pub(crate)`, `pub(super)`, or `pub(in path)` — an everyday visibility
modifier, not a contrived one, so a `#[test] pub(crate) fn ...` deleted
with no trailer passed silently. The round judged it non-blocking (this
repository has no `.rs` files yet, and a gap of this shape is the same
tier as the already-accepted one), but it was a one-line regex fix rather
than only a documentation addition, so it was fixed rather than deferred:
`pub(\([^)]*\))?` in place of a bare `pub`, verified against `pub(crate)`
and `pub(in crate::tests)` both being caught, plus the full regression set
once more. Both gates are wired into the local `.git/hooks/pre-commit` and
`commit-msg` scripts alongside M-1.6's and M-1.9's, following the same
precedent: local wiring now, `.pre-commit-config.yaml`'s portable version
in M-1.14.

**M-1.8** wires non-negotiable 5 (sans-I/O) and the crate-layering rule
`code-structure.md` rule 1 and `architecture.md` both already cited a gate
for. `check-layering.sh` reads each crate's `Cargo.toml` well enough to
answer one question — which workspace crates does this one depend on at
runtime — without a real TOML parser: it tracks section headers and, inside
`[dependencies]`, treats a bare `name = ...` line as a dependency and a
`[dependencies.name]` header as one too, careful not to mistake that
sub-table's own `path = ...` field for a second dependency named `path`.
`oqueue-broker` and `bin/oqueue` (package `oqueue`) are the two named
composer exceptions; `oqueue-testkit` may never appear in any crate's
`[dependencies]` regardless of composer status. `check-sans-io.sh` greps
every library-crate `.rs` file for three literal-identifier patterns — a
concrete socket type, a real clock read, an object-storage SDK call — each
exempting `oqueue-broker` (the I/O shell) and, for the object-storage
pattern only, `oqueue-store` (the `ObjectStore` seam's implementor).

Both were built against a fake scratch workspace rather than trusted from
reading alone, since the real repository has no crates yet for either gate
to exercise: a five-crate `Cargo.toml` tree (including a genuine
`oqueue-testkit` crate, without which the dev-only check cannot fail —
caught on the first attempt, when an empty scratch workspace made the
testkit-in-runtime-deps case pass for the wrong reason, no `oqueue-testkit`
crate existing at all to be flagged) confirmed every rule in both scripts:
a sideways dependency, `oqueue-testkit` leaking into `[dependencies]`, the
table-header dependency form parsing correctly, a socket type and a real
clock read and an S3 SDK call each flagged in an ordinary library crate,
and `oqueue-broker`/`oqueue-store` correctly exempt from exactly the
patterns their own role requires and nothing more (`oqueue-store` calling
the S3 SDK passed; `oqueue-store` opening a raw `TcpStream` still failed).

⚠️ Review found two real gaps the happy-path testing above did not reach,
each severe enough on its own that neither was left to a "does not catch"
disclosure. `check-layering.sh`'s flow-form parser matched `name = ...` but
not Cargo's dotted-key shorthand `name.workspace = true` — an ordinary,
idiomatic way to centralize dependency versions across a workspace, not an
obscure one — so a genuinely sideways dependency written that way passed
clean, and the same gap would have let `oqueue-testkit` leak into a runtime
`[dependencies]` block undetected. **Blocking**, because it defeats this
task's first acceptance criterion outright for a realistic Cargo idiom, not
a contrived one. Fixed by matching the key up to its first `.` rather than
requiring the whole thing to be a bare identifier, verified against the
exact `oqueue-index.workspace = true` sideways case and an
`oqueue-testkit.workspace = true` runtime leak, both now caught. Second,
`check-sans-io.sh`'s socket pattern used `\b` word-boundary anchors — the
exact portability hazard `check-drift.sh` had already named and worked
around one task earlier, reintroduced here with no note explaining why it
was safe this time (it was not). **Major**, since GNU and BSD grep disagree
about `\b` and portability.md makes macOS a first-class development
platform this gate has to run on. Fixed by replacing it with an explicit
non-identifier-character (or line-start/end) boundary — a different,
equally portable answer to the same problem `check-drift.sh` solves by
dropping any boundary at all and accepting the substring false-positive
risk instead, which the second review round caught this file
misdescribing as identical to `check-sans-io.sh`'s approach when the two
scripts actually take opposite ones. Corrected here rather than left
standing. Verified
against start-of-line, end-of-line, and ordinary matches.

A second review round passed both fixes and found two further minor
issues, both fixed rather than deferred. `check-layering.sh` still has one
disclosed gap left: a dependency renamed via Cargo's `package` key (e.g.
`sideways = { package = "oqueue-other", path = "..." }`) records the local
alias as the dependency name and never resolves what it actually points
at, so a renamed sideways dependency is invisible to every rule in the
script rather than checked under its real identity — nothing in this
workspace's conventions calls for renaming an internal crate, so this is
added to "What this does not catch" as a real, undisclosed-until-now gap
rather than fixed outright, since closing it needs the value-parsing this
script deliberately avoids. And this file's own paragraph above originally
claimed the sans-I/O fix reused "the same" boundary technique
`check-drift.sh` uses — false: `check-drift.sh` uses no boundary at all,
by design, and says so in its own header; the two scripts solve the `\b`
problem in opposite ways. Corrected above.

Both gates currently skip cleanly on this repository (`has_rust()` for the
layering gate, an empty `git ls-files` glob for sans-I/O) and stay
unexercised here until M0.

**M-1.10** wires non-negotiable 6. `check-core-contract.sh` extracts each
`pub trait Name { ... }` block by brace depth and reduces it to a **set**
of normalized `fn` signatures — a required method captured up to its `;`,
a provided method's default body skipped by depth so it is never
misread as further signatures — then compares that set between HEAD and
the staged index for every changed `.rs` file. A trait whose set differs
requires every file that currently implements it (`impl Name for` found
anywhere in the tracked tree) to also be part of this commit's changed
files, and an ADR under `docs/internal/product/decisions/` in the same
commit. Doc 19 §3.2's own limits are kept deliberately: the gate does not
try to verify an implementor's body actually matches the new signature,
only that its file is present in the commit — the same "checks only what's
mechanical" posture `check-drift.sh` and `check-tests-kept.sh` already
state for their own gates.

⚠️ Built and tested against a scratch git repository with real commits
(unlike M-1.8's gates, this one needs history — it compares HEAD against
the index, not two states of a working tree), and the manual testing found
a real defect before any review did: `index_content()` built the staged
-index git reference as `git show "::path"` — a doubled colon, from
passing `":"` as the ref into a helper that already appends `:` itself —
which fails with exit 128 on every call. The failure was swallowed to
`None` by the same helper, and every caller already treats `None` as "this
file doesn't exist," so the result was silent and wrong in the most
dangerous direction: every trait in a changed file looked like it had
vanished entirely rather than changed, `changed_traits` still populated
correctly from the `HEAD`-side comparison, but the *new* side read as
empty — coincidentally still detecting "a change occurred" while reporting
the wrong implementors (none, since `impls_by_trait` was built entirely
from the same broken helper and came back empty too). Two of the four
manual test cases run before this was found happened not to exercise the
one code path where it mattered, which is the same lesson M-1.7's own
retrospective already recorded once this task: a test that looks like it
covers a case can silently cover a different one. Fixed by not
concatenating two colons; reverified against the full case set — sideways
implementor missing, ADR missing, both present, a doc-comment-only edit, a
default-body-only edit, an unrelated function added to the same file, and
a brand-new trait in a brand-new file — all before this was ever sent to
review.

⚠️ Review found a second defect in the same shape as the first, one layer
further in. `pub trait Name` and `impl Name for` are found by plain
substring search over the raw file text, with no comment stripping — so a
trait name mentioned in a *later* comment (a stale doc example,
commented-out old code, a migration note) overwrote the real trait's entry
in the dict this builds, because matches are visited in source order and
the last one wins. Reproduced: a real, breaking signature change to a
`pub trait`, with the trait's old shape quoted verbatim in a trailing
comment, made the gate report `ok no pub trait's method set changed` —
exactly the failure mode `contracts.md` rule 13 promises cannot happen
("editing a doc comment on a trait is not a contract change"), except here
a comment made a *real* change invisible rather than a non-change visible,
the opposite direction and the more dangerous one. Fixed with a
best-effort comment stripper — `/* */` blanked out nesting-aware, `//` to
end of line, skipping a `//` that falls inside a simple `"..."` string —
applied before both the trait-signature extraction and the implementor
scan, since both are vulnerable to the identical shadowing. Not
string-literal-aware across a multi-line raw string; documented in the
script rather than solved, since a raw string inside a trait's own
signature is rare enough to accept. Reverified against the exact
comment-shadowed reproduction plus the full original case set once more.

⚠️ A second review round found the string-tracking half of that fix had the
opposite bug, reached through a different case than the one just closed: a
char literal holding a double quote, `'"'` — plausible in any wire-protocol
codec comparing a byte against a quote character, not contrived — has no
matching close on the line, so the scanner stayed "inside a string" for
the rest of the line with nothing to end it, and a genuine trailing `//`
comment was never cut. That comment's text then leaked into the real
trait's signature set exactly like round 1's defect, just through the
string-tracker instead of the missing comment-strip. Measured in the
*opposite* direction from round 1 though: every case tried turned a
non-change into a false `CHANGED`, never a real change into a false
`unchanged` — still wrong, but not the more dangerous of the two
directions this task exists to prevent. Fixed narrowly for the one
ambiguous case (`'"'` specifically), not with a general char-literal
lexer, and reverified against the exact reproduction plus the full case
set including the round-1 comment-shadow scenario and nested block
comments.

⚠️ **A third review round found the two fixes above had never actually been
combined correctly, and it was the dangerous direction again.** Both
rounds patched their own pass in isolation — the block-comment pass ran
first, over the raw file, with no string awareness at all; the string
tracker ran second, per line, over whatever the first pass had already
produced. An unmatched `/*` inside an ordinary string literal (a MIME
type, a glob pattern, a log message — plausible in ordinary code, not
adversarial) had nothing to stop it, because the pass that would have
recognized "this is a string" ran *after* the pass that blanks comments.
The result: everything from that `/*` to end of file was blanked, silently
deleting every trait declaration after it — including ones with a real,
breaking signature change — from extraction. Reproduced two ways: the
unmatched `/*` immediately before the changed trait, and buried in an
unrelated helper function earlier in the file; both made a genuine
breaking change to `pub trait Framer` disappear entirely, gate reporting
`ok` and exiting 0. The doc comment's own claim that this exposure was
"confined to a trait's own signature or an `impl ... for` line" was itself
wrong — the two-pass structure meant the blast radius was the rest of the
file, and the review that found this said so explicitly rather than taking
the disclosed-limitation framing at face value.

Fixed by replacing both passes with one: a single left-to-right scan
tracking string, block-comment, and line-comment state together, checked
in that order — a block comment can only *begin* when the scanner is not
already inside a string, which is what makes an unmatched `/*` inside a
string literal inert instead of catastrophic. The char-literal special
case from round 2 carries over unchanged. Reverified against every case
from all three rounds in one pass: both unmatched-`/*`-in-a-string
reproductions (now correctly detected as changed), the round-1
comment-shadow case, the round-2 `'"'` false-positive case, escaped
single-quote and escaped-backslash char literals each hiding a real
change behind a trailing comment, a nested block comment mentioning an
unrelated trait, and a doc-comment-only edit.

**M-1.11** closes the non-negotiables list except rule 3. `check-unsafe.sh`
enforces two things: `unsafe` outside `oqueue-buf`/`oqueue-codec`/
`oqueue-checksum` fails outright, and every `unsafe fn`/`unsafe { ... }`
site inside those three needs an immediately-preceding `// SAFETY:` comment
whose text has a baseline entry in the new `baselines/unsafe.txt`, id =
`sha256(file + "\0" + the comment's own text)` truncated to 12 hex
characters — the same shape `baselines/review.txt` already established for
argued findings and `testing.md` rule 17's mutants baseline, chosen for
the identical reason: keyed on content, not a line number, so an unrelated
edit above a baselined block never invalidates its entry. `unsafe impl
Send`/`unsafe trait` marker forms are confined to the three crates like
everything else but are not required to carry a SAFETY comment — doc 18
§5.7's six conditions are framed around a block or function with a
precondition to state, and a marker trait has none.

Built and verified against a scratch git repository: unsafe confined with
a baselined SAFETY comment passes; the identical block with no baseline
entry yet fails and prints the exact id to add; unsafe in a non-allowed
crate fails; an `unsafe fn` with no SAFETY comment at all fails; an
`unsafe impl Send` marker needs no comment but is still confined; and an
unstaged baseline entry does not satisfy the gate, read from the index for
the same reason `baselines/review.txt` is.

Round 1 found two real defects and one piece of unnecessary weight:

- **blocking**: the per-file read only caught `OSError`. A non-UTF-8 byte in
  a scanned file raises `UnicodeDecodeError`, which is not an `OSError` —
  the scanner died with Python's own exit code 1, indistinguishable on the
  bash side from "no violations found", and every file after the crashing
  one in scan order was silently never checked. Reproduced with a file
  containing one raw `\xff` byte. Fixed two ways: the read is now wrapped in
  `except (OSError, UnicodeDecodeError)`, so one unreadable file is skipped
  rather than killing the run, and the whole scan body is wrapped in
  `def build(): ...` called from a top-level `try/except Exception` that
  maps any *other* unexpected exception to a distinct exit code (3),
  matching `build-index.sh`'s own established crash-safety pattern for the
  identical reason. A skipped file is not silent either: it now prints as a
  `warn` naming the path, because a file the scanner could not read is a
  file not scanned for unsafe, and `lib.sh`'s own contract is that a gate
  states what it checked even when it passes.
- **major**: `UNSAFE_SITE` was matched with `re.search` per line, so
  `unsafe` and `{` on separate lines — valid, ordinary Rust that `rustfmt`
  does not forbid — evaded the SAFETY-comment requirement entirely.
  Reproduced with `unsafe` alone on one line and `{` on the next. Fixed by
  matching against the whole file's text once with `re.finditer` and
  mapping each match's start offset back to a line number via
  `bisect.bisect_right` over a precomputed table of line-start offsets,
  rather than searching line by line.
- **minor**: `require_sha256 || finish` was dead weight — the id is computed
  in the embedded Python via `hashlib`, never shelled out to `sha256sum`, so
  the tool-presence check gated nothing. Removed.

Reverified the full original suite after the fix (well-formed pass, missing
baseline, missing SAFETY comment, unsafe in the wrong crate, marker-impl
exemption, unstaged baseline) with no regression, plus the two new cases
above and a forced-exception run confirming the crash path fails loudly
with a distinct message rather than reporting a false "ok".

Round 2 found two more real defects, both in the round-1 fix itself:

- **blocking**: the SAFETY-comment lookback anchored to the line of the
  `fn`/`{` token (`m.start(2)` in the round-1 regex), not the line of the
  `unsafe` keyword. For a split-line site — the exact shape round 1's major
  fix was written to detect — the line immediately above the detected site
  was the `unsafe` line itself, not the comment above it, so a correctly
  placed `// SAFETY:` comment above `unsafe` was invisible to the lookback.
  Reproduced: `// SAFETY: ...` above `unsafe`, `{` on the next line, a
  correct staged baseline entry for that exact comment text — failed with
  "no SAFETY: comment" anyway. The single-line form (`unsafe {` together)
  passed with the same baseline entry, confirming the anchor was the only
  variable. Fixed by capturing the `unsafe` keyword itself as its own group
  and anchoring the lookback (and the reported line number) to its
  position, not the trailing token's.
- **major**: rule 1's confinement match was still the original whole-word
  `unsafe`, unchanged since before round 1 — it matches the English word in
  prose, not only the Rust keyword. Reproduced: a file outside the three
  crates containing only a doc comment reading "this crate ... never uses
  unsafe code, unlike oqueue-buf which does" failed the gate with no
  `unsafe` keyword anywhere in it. Not a security bypass (errs toward
  over-blocking), but likely to produce spurious failures once library
  crates carry real prose about their own safety posture. Fixed by
  requiring one of the tokens that actually follows the keyword in Rust
  syntax (`fn`, `impl`, `trait`, `extern`, or `{`), matched over the whole
  file's text so a confinement violation split across a line break (the
  same shape round 1 fixed for the SAFETY-required subset) can't evade rule
  1 either — the bash-side `grep -n` rule 1 previously used could not have
  spanned lines even if its pattern had required a trailing token, which is
  why detection for both rules now lives in the one whole-text Python scan
  rather than half in `grep` and half in Python.

Reverified the full original suite again after both fixes, plus a
correctly-annotated split-line site (now passes), a disallowed-crate file
containing only prose mentioning "unsafe" (now passes), and a
disallowed-crate site split across a line break (still correctly caught,
confirming the rule-1 move into Python didn't lose the round-1 span
coverage) — no regression anywhere.

Round 3 found two more real defects, both about legitimate Rust the earlier
rounds' fixes had not accounted for:

- **blocking**: the SAFETY lookback stopped at the first line that was
  neither a `//` comment nor blank — an attribute (`#[inline]`, `#[cold]`,
  `#[target_feature(...)]`) between a correct `// SAFETY:` comment and the
  `unsafe fn`/`unsafe {` it documents made the comment invisible, exactly
  the pattern doc 18 §5.7 and the performance standard recommend for
  hot-path unsafe functions. Reproduced with a `// SAFETY:` comment,
  `#[inline]`, then `unsafe fn` — failed despite the comment being present
  two lines above. Fixed by marking every line that belongs to an attribute
  (tracking `[`/`]` depth so a multi-line attribute, e.g. a wrapped
  `#[cfg_attr(...)]`, is covered too — the same forward `in_attr`-style scan
  `check-tests-kept.sh` already uses for its own attribute lines) and having
  the lookback skip over those lines rather than stopping at them.
- **major**: rule 1's confinement regex required one of `fn`/`impl`/`trait`/
  `extern`/`{` after `unsafe`, which does not include edition-2024's
  `#[unsafe(no_mangle)]` attribute-wrapper syntax — rustc itself requires
  the wrapper for `no_mangle`/`export_name`/`link_section`, so this is
  genuine unsafe-introducing syntax, not an oversight to exempt. Reproduced:
  a disallowed-crate file containing only `#[unsafe(no_mangle)]` over an
  `extern "C" fn` passed with zero violations reported. Fixed by adding
  `\(` to the site regex's trailing-token alternation — `unsafe` is a
  reserved word, so `unsafe(` cannot be anything but this wrapper form, no
  further disambiguation needed. Treated as confinement-only like `impl`/
  `trait`/`extern`, not SAFETY-required: it is an attribute annotation, not
  a block asserting one inline runtime invariant.

Reverified the full suite a third time after both fixes (12 cases: the six
from round 1 unchanged, plus the round-2 split-line and prose cases, plus a
multi-line-attribute-annotated site now passing, an attribute-only site
with no SAFETY comment still correctly failing, and the `#[unsafe(...)]`
wrapper correctly confined) — no regression.

Round 4 found one more real defect in round 3's own fix, plus a doc/code
mismatch:

- **blocking**: `attribute_lines()`'s depth tracker counted every `[`/`]`
  character on a line, including ones inside a string literal that is
  itself an attribute argument's value. An attribute whose string value
  contains an unbalanced `[` (`#[doc = "array[ unmatched bracket..."]`, a
  plausible `#[serde(rename = "items[0]")]`) pushed `depth` permanently
  above zero, so `in_attr` never cleared — every line for the rest of the
  file, including later genuine `// SAFETY:` comments, was silently
  swallowed into the attribute-skip set instead of being read as a comment
  or used as a boundary. Reproduced with exactly the `#[doc = "array[...`
  case; a correctly placed SAFETY comment two lines later failed the gate.
  Fixed by stripping ordinary double-quoted string content
  (`"(?:\\.|[^"\\])*"`) from each line before counting brackets — the same
  string-awareness discipline `check-core-contract.sh`'s own brace-depth
  scan had to learn the hard way. Disclosed, not chased further: a raw
  string or an unterminated (non-compiling) string literal inside an
  attribute can still miscount, the same category of accepted gap
  `check-core-contract.sh` already discloses for itself.
- **minor**: the header claimed the SAFETY/site association tolerated
  "blank lines ... in between", but the lookback actually stops at the
  first blank line — it never skipped them, only comments and attributes.
  The stricter code was correct (an accidental blank-line gap should not
  associate a comment with a much later, unrelated site); the header was
  wrong. Fixed by correcting the header's wording, not the behavior.

Reverified the full suite a fourth time (14 cases: the twelve above, plus
the bracket-in-attribute-string case now passing, plus a blank line
between comment and site still correctly failing) — no regression.

Round 5 found the same defect *class* as round 4 recurring through a
second, unstripped path, plus one more prose false positive:

- **blocking**: `attribute_lines()`'s bracket counter had no concept of
  `/* ... */` block comments. A block comment containing commented-out,
  attribute-shaped text with an unbalanced bracket count (kept-for-context
  reference code, a rejected draft) set `in_attr` and never cleared it, the
  identical whole-file cascading failure round 4 fixed for string literals,
  reached by an unstripped comment instead. A companion case, a single
  attribute containing a bracket inside a char literal
  (`#[my_tool_attr('[')]`), showed the round-4 fix (string-only stripping)
  was one instance of the general problem, not the problem itself.
- **major**: the `\(` alternative added in round 3 for `#[unsafe(...)]`
  reopened round 2's prose defect for that one token -- `unsafe(` matched
  inside an ordinary `//` comment that merely named the syntax by way of
  explaining a crate didn't need it, the same class round 2 eliminated for
  the other trailing tokens but never re-checked when `\(` was added.

Both are the same root cause surfacing a third and fourth time: matching
and counting brackets against raw source text cannot distinguish real code
from a comment or a string that merely contains code-shaped text, and
every incremental, single-shape fix (strings in round 4, now attributes
found via comments) left the next shape uncovered. Fixed by stopping the
one-shape-at-a-time patching and porting `check-core-contract.sh`'s
tested `strip_comments()` state machine wholesale, extended to also blank
string and single-quote-char-literal *interiors* (that file only ever
needed comments blanked; this gate's regex matches on syntax shape, which
strings and char literals can just as easily contain). `SITE` and the
attribute-bracket counter now both run against this one stripped text
instead of the raw one; the SAFETY-comment lookback still reads the raw
lines, since it needs the real comment text to hash, not a blanked one.
Char-literal recognition uses a bounded lookahead for the two shapes that
occur in practice (`'x'`, `'\x'`) rather than a full lifetime/char-literal
disambiguator -- the same "narrow special case, not a full lexer" posture
`check-core-contract.sh` already established for the identical ambiguity
in its own scanner, and it turned out to subsume that file's separate
`'"'`-char-literal special case for free, since char-literal detection
here happens at the opening quote, before the interior `"` is ever visited
on its own.

Reverified the full suite a fifth time (17 cases: the fourteen above,
plus the block-comment-with-unbalanced-bracket case now passing, a
bracket-in-char-literal attribute now passing, and `unsafe(no_mangle)`
named inside a `//` comment outside the allowed crates no longer failing)
— no regression.

Round 6 found the most severe defect of the six rounds: a real false
*pass* (every prior round's dangerous-direction findings were near
misses, caught before shipping; this one was live in the diff sent to
review), plus a second false-rejection defect in the same family as
rounds 4-5, plus a cosmetic one:

- **blocking**: `strip_for_scan`'s `'string'` state applies
  backslash-escaping to every double-quoted string uniformly, but raw
  strings (`r"..."`, `r#"..."#`) have no escape sequences in real Rust.
  Reproduced: `r"\"` (a common shape -- any Windows-style path literal, or
  a regex fragment ending in a literal backslash) ends its content with a
  backslash directly before the closing quote; the escape-tracking state
  interpreted that quote as escaped and kept consuming -- silently
  blanking every real character after it, including a genuine
  `unsafe { ... }` block outside the three allowed crates, until an
  unrelated later `"` or end of file. `check-unsafe.sh` reported `ok` on a
  file that plainly violates rule 1. This is the dangerous direction
  non-negotiable 7 exists to prevent, and the disclosed "what this does
  not catch" note previously understated it by lumping raw strings in
  with "does not compile" cases -- this one compiles and is ordinary.
  Fixed by recognizing the `r`/`br` prefix and pound-count via a backward
  lookahead from the opening quote (mirroring how rustc's own lexer
  resolves it) and closing on the real rule: `"` followed by exactly that
  many `#`, no escaping in between. Verified against `r"\"`,
  `r#"[a-z]+\"#`, and `br"C:\some\path\"` (pound-delimited and byte-string
  forms) each followed by a real unsafe site, and confirmed an ordinary
  escaped string (`b"a\\\"b"`) is unaffected.
- **major**: `attribute_lines()` tracked bracket depth as a whole-line net
  count (`ln.count('[') - ln.count(']')`), so a line where an attribute
  closes and unrelated real code follows on the *same* line
  (`#[derive(Debug)] struct Padding([u8; 4]);`) still balanced to zero for
  the line as a whole (the struct's own `[u8; 4]` also nets to zero) and
  the entire line -- including the struct declaration -- was marked
  transparent. The SAFETY lookback then skipped straight over it,
  letting an unrelated, far-above `// SAFETY:` comment attach to an
  unsafe site it never described: a false pass on the baseline-coverage
  check specifically, the same dangerous direction as the blocking
  finding, just one level down. Reproduced exactly as described. Fixed by
  scanning each attribute line character by character to find the exact
  column its own bracket closes at, and only marking a line transparent
  when nothing but whitespace follows that column -- a line mixing an
  attribute with real code is now left unmarked and acts as an ordinary
  boundary.
- **minor**: the `char_lit` state's fallback branch blanked every
  non-escape, non-closing character to a plain space, including `\n`,
  contradicting the function's own documented "every newline is preserved"
  invariant. Unreachable in practice (a bare newline inside `'...'` is not
  valid, compiling Rust), but fixed for free while already in this code:
  now preserves `\n` like every other blanking branch does.

Reverified the full suite a sixth time (20 cases: the seventeen above,
plus the raw-string-masking case now correctly failing on its real
`unsafe` site, the attribute-with-trailing-code case now correctly
requiring its own SAFETY comment, and two extra probes -- a
pound-delimited raw string and an escaped byte string, both followed by a
real unsafe site that is still correctly caught) — no regression.

Round 7 asked the reviewer to scrutinize the string-handling machinery
hardest, since round 6 had found a real false pass there; that machinery
held. It found a second false pass instead, in code unchanged since round
1:

- **blocking**: the main scan loop deduplicated by line number --
  `if i in seen_lines: continue` -- on the mistaken assumption that a line
  holds at most one relevant `unsafe` site. `unsafe fn f() { unsafe { ... }
  }`, or an edition-2024 `#[unsafe(no_mangle)] pub unsafe extern "C" fn
  foo() { unsafe { ... } }`, puts two or more genuinely distinct `unsafe`
  occurrences on one line, and every one after the first was silently
  never examined for a SAFETY comment or baseline entry -- a real,
  uncommented, unbaselined unsafe block reported as compliant.
  Reproduced both ways: a nested `unsafe fn`/`unsafe {}` pair sharing a
  line with only the outer site's comment baselined passed cleanly, and
  the FFI-export shape above with an entirely empty baseline file also
  passed. `SITE.finditer` never produces two matches for the same keyword
  occurrence -- each match consumes its own boundary character and
  scanning resumes strictly after it -- so every match was already a
  genuinely distinct occurrence; there was nothing to legitimately dedup,
  and the dedup itself was the bug. Fixed by removing it. A shared
  physical line means sites sharing that line also share whichever
  comment sits directly above the line, at this gate's line-level
  granularity (unchanged, documented) -- two sites needing two different
  justifications is exactly what pulling the inner block onto its own
  line already achieves, same as any other case where this gate's
  granularity asks for a line split.

Reverified the full suite a seventh time (both repros above now correctly
failing, plus the full twenty-case suite unchanged) — no regression.

Round 8, asked to scrutinize the whole file for any remaining way a real
site could be silently missed given the last two rounds' pattern, found
one more real false pass, plus confirmed round 7's fix introduced no new
false positive:

- **blocking**: `unsafe extern` is ambiguous by itself -- `unsafe extern
  "C" fn foo() { ... }` is a real function definition with a body and
  exactly one inline invariant to state (indistinguishable in shape from
  `unsafe fn`); `unsafe extern "C" { fn foo(); }` is a foreign block, a
  list of declarations with nothing to assert. `SITE` only ever captured
  the token immediately after `unsafe`, which is `extern` either way, so
  every `unsafe extern "ABI" fn` site was silently classified with the
  marker-exempt block form and never checked for a SAFETY comment or
  baseline entry at all -- the header's own "what this does not catch"
  section already correctly scoped the disclosed exemption to *blocks*
  only, but the code did not actually enforce that distinction. Reproduced
  with a bare, uncommented `pub unsafe extern "C" fn my_callback(...)` in
  an allowed crate against an empty baseline: reported `ok`. Fixed by
  peeking past the optional ABI string literal for the real continuation
  (`fn` vs `{`) and reclassifying accordingly; an unrecognized or
  ambiguous continuation resolves to requiring a SAFETY comment rather
  than to the exempt form, since a false rejection here costs a second
  look and a false exemption costs nothing and is invisible -- the same
  fail-safe default this file has settled on for every dangerous-direction
  fix from round 6 onward.
- Confirmed, not a defect: round 7's dedup removal does not introduce
  spurious double-reporting for a genuinely single site --
  `SITE.finditer` cannot produce two matches for one keyword occurrence,
  verified again directly.

Reverified the full suite an eighth time (21 cases: the twenty above,
plus five extra probes around `unsafe extern` -- a bare uncommented
extern fn now correctly failing, the same fn with a comment now passing,
an extern *block* still correctly exempt, a bare `extern fn` with no ABI
string still correctly requiring SAFETY, and an extern fn outside the
three allowed crates still correctly confined) — no regression.

Round 9, asked to hunt specifically for a fifth dangerous-direction false
pass given the pattern of the previous four (each in a different
mechanism: string handling, attribute-line tracking, line dedup, extern
disambiguation), reasoned through Rust's actual function-qualifier grammar
(`const|async → unsafe → extern → fn`, meaning `unsafe` is always
immediately followed by `extern` or `fn` for a function item, so no legal
function-definition shape exists that `SITE`'s trailing-token set doesn't
already recognize) and tested adversarial `unsafe extern` shapes directly
against the real script: a function-pointer type field, a malformed
double-ABI-string, a no-ABI-string form, an ABI string containing an
escaped quote, and an `unsafe impl` sharing a line with a genuine
`unsafe { ... }` block. **Pass, no findings** -- the first round of nine
to return clean.

**M-1.9** is new to oqueue and has no precedent to port. The mechanism is in
[docs/researches/21](../../researches/21-ai-development-loop.md) §5. The
reviewer must not receive the author's reasoning; separation of invocation is
not separation of information.

It converts the review from a claim into an artifact, which is the one thing
non-negotiable 3 cannot do for test claims. The verdict lives at
`target/review/<sha256-of-staged-diff>.json` and the gate recomputes the hash
rather than reading a filename, so amending a byte after review invalidates it.

⚠️ **What the gate does not prove: that the reviewer was not the author.**
Nothing in the artifact carries provenance, and a determined author can write
the JSON. What it buys is that the verdict is *about these exact bytes*, which
is the property that was missing; isolation is bought by the harness
(`.claude/agents/reviewer.md`), not by the script. Saying so plainly is the same
discipline as rule 3.

⚠️ **`review.sh` deliberately does not spawn the reviewer.** No script can start
an agent in a way that works across tools, and hard-coding one vendor's CLI
would put a procedure in the enforcement path — the fork the skills exist to
avoid. The script owns the hash, the packet, the schema, and the artifact; the
agent owns the judgement.

The gates the packet reports as already passed are **discovered** — every
`scripts/check-*.sh` minus a short exclusion list carrying a reason per entry —
so a gate added by a later task appears without anyone remembering to add it.

Nine negative cases ran before this landed: no artifact; a verdict claiming a
different diff; a finding with no failure scenario; blocking findings with a
`pass` verdict; an invalid severity; prose instead of JSON; **one byte amended
after review**, which is the acceptance criterion; an artifact hand-edited and
filed under the wrong hash; and a blocking finding resolved by arguing it in the
baseline. All nine behaved.

⚠️ They also found a defect no reviewer would have: `python3 …; rc=$?` under
`set -e` kills the script at the failing command, so the `fail` line never
prints and the exit code is python's rather than the `1` every gate here
promises. `build-index.sh` had the identical bug and **survived its own negative
test**, because the `PROBLEM` lines still printed and only the summary was
missing — a gate failing for the right reason with the wrong message and the
wrong code. Corrected in a follow-up commit naming M-1.33, not by amending it,
because that commit is pushed.

⚠️ **A fresh clone is not yet enforced.** `.git/hooks` is not tracked, so the
wiring that makes "amending one byte fails the commit" true lives on one
machine. The script is correct and the hook is installed here; **M-1.14** is
what makes a clone inherit it, and until then a new checkout gets the gate only
by installing the hook by hand. M-1.6 left the same hole for
`check-commit-msg.sh`.

Review found eight things in one round, and two of them were the kind no test
would have: the packet ran the deterministic gates **against the working tree
while hashing the index**, so staging a broken change and restoring the file
made it assert a gate passed for bytes that were not being committed — with the
reviewer explicitly told not to re-check them. And `sha256sum` was called
unguarded, which on macOS (a first-class platform under portability.md rule 2)
meant `command not found`, exit 127, or worse: `context` printed a one-line
packet and **exited 0**, so a wrapper checking `$?` would proceed to review
nothing. Gates now run against a materialised copy of the staged tree, and
`sha256_stdin` falls back to `shasum`.

Three of the rest were claims that were not true of the code — the same class as
M-1.34's false comment. The baseline's header told the reader that
`review.sh record` prints the finding id; it did not, so arguing a finding meant
opening the JSON by hand. `record` now prints them. The baseline also accepted
an id with no reason at all, suppressing a blocking finding while recording
nothing about why. And a `major` finding could not be recorded as
`changes-requested`, so a reviewer who found two genuine major defects had to
file them as a `pass`, after which they lived only in a gitignored file.

⚠️ A second round found the sharpest defect of the milestone so far, and it was
an **incentive**, not a bug. The argued-findings baseline was read from the
working tree, so appending a line *without staging it* suppressed a blocking
finding: the staged diff was unchanged, the verdict stayed valid, the gate
passed — and the commit recorded the pristine template, so the milestone-
boundary reader the file's own header demands would see an empty argued-list.
Staging the entry properly moved the hash and invalidated the review, so **the
honest path was punished and the trace-free one rewarded.** The baseline is now
read from the index. Arguing a finding is part of the commit, which forces one
more review round. ⚠️ **Superseded below:** that round does not reliably
terminate, because the id is derived from the reviewer's prose.

The same round found three more holes of the shape this milestone keeps
producing — a gate that reports success while enforcing nothing. The
materialised staged tree sat under `target/`, hence still inside the work tree,
so `git rev-parse` there resolved to the real `.git`: a gate written as
`git ls-files … | xargs grep` would iterate zero files, exit 0, and be reported
as passed. ⚠️ **Superseded below:** the first fix moved it out of the
repository, which broke a different standard. `which-standards.sh` failing killed the packet mid-write with
its explanation sent to `/dev/null`, leaving plausible-looking Markdown with no
diff in it. And `check-reviewed.sh` read the verdict, printed it, and never
acted on it, so `changes-requested` passed with an `ok`.

A third round found that the *fix* for the materialised tree had traded one
standard for another: moving it to the system temp directory satisfied the
git-discovery problem and broke build.md rule 19, which keeps scratch out of a
RAM-backed tmpfs — and left a full copy of the index there on an interrupted
run with nothing to reap it. Both constraints are kept now: the tree is under
`target/tmp` with a deliberately invalid `.git` planted at its root. Measured:
without it a `git ls-files` gate reports **0 files and exit 0**; with it, 128.

⚠️ It also found the termination argument for arguing a finding was **unsound**,
and that one is recorded rather than fixed. The id is `sha256(file + summary)`,
so it survives unrelated edits — but the extra round that arguing forces is a
different invocation looking at a different diff, and a reviewer wording the
same defect differently produces a different id, leaving the staged argument
dead. Not a livelock, since fixing is always available; the cost is that the
baseline can accumulate several ids for one defect, which blunts the one signal
it exists to carry. If that starts happening, the id needs to derive from
something stabler than prose.

⚠️ A fourth round found the **third instance of the `set -e` bug class in this
milestone**, this time introduced by the refactor that fixed the second round's
findings. Folding the two baseline loops into one function produced
`argued "$id"; a=$?` — a bare call in an untested context — so the moment a
blocking finding was *not* argued, the shell died before the `fail`, the
remedy, and the summary. The gate refused the commit with **no output at all**,
and with exit 2 for an unusable id rather than the 1 every gate here promises.
The failure path of the check was the one path that could not report, and it
broke lib.sh's contract that a run reports every violation rather than only the
first.

⚠️ **The tell is a bare call whose non-zero return is meaningful**, and it is
worth naming because the milestone has now produced it three times in three
different scripts. It is also a lesson about how the refactor was tested: the
new paths were exercised and the path that used to work was not.

⚠️ Naming the tell was not enough: a fifth round found **two more instances in
the same diff that named it**. `ws_err="$(mktemp …)"` sat one line above the
guard written to catch precisely this, so an unusable `TMPDIR` killed `context`
and emitted the truncated packet all over again — and it contradicted this
file's own argument for keeping scratch out of the system temp directory.
`known="$(known_task_ids)"` was the same shape through a pipeline: a backlog
whose table had no rows returned 1 and killed the gate with no output, while a
*missing* backlog was handled gracefully — the two empty states behaved
oppositely, and the graceful one was the anticipated one.

So the fix went into `lib.sh` rather than the call site, and the whole script
set was swept for the pattern rather than patching the two that were reported.
⚠️ One instance is knowingly left: `check-commit-msg.sh`'s
`grep -v '^#' "$MSG_FILE" | head -1` dies the same way when a commit message is
entirely comments. It belongs to M-1.6, so it is tracked as **M-1.35** and
corrected in its own commit rather than folded in here — prose inside a done
task's retrospective is not a record the `next-task` skill can see, and
AGENTS.md is explicit that out-of-scope work becomes a backlog task.

⚠️ Later rounds found the pattern twice more, in places the sweep had not
reached because they did not look like calls: `|| true` on `known_task_ids`
fixed the death but produced a **silent skip** — the membership check stopped
running and the gate still printed `ok`, which lib.sh's own header names as the
failure to avoid — and `shift 2` in the option loop killed the script with zero
bytes on both streams whenever a flag arrived without its value, which is what
`--task $UNSET_VAR` interpolates to.

Counting the two follow-up commits, this class accounts for **six of the
defects found in M-1.9**. Every one of them made a gate report something other
than what it checked, and none of them was visible from reading the diff — they
were all found by running the failure path.

**M-1.12** has no constant yet. Two minutes is a common figure for a pre-commit
budget, but it is usually written as a comment and never measured, and oqueue's
dependency graph is heavy. Derive it from measurement once M0's workspace
exists; until then the task is blocked rather than guessed.

**M-1.14** ⚠️ the CI adaptation that is easy to miss: with no pull requests, any
gate triggered by one silently never runs. `.pre-commit-config.yaml` wires
every existing gate script as a local hook (`default_install_hook_types:
[pre-commit, commit-msg]`, so one `pre-commit install` gets both stages),
which is what makes a fresh clone inherit the same enforcement this repository
only ever had by hand-copying scripts into `.git/hooks` — a gap M-1.9 and
M-1.6 each left open in turn. `.github/workflows/gates.yml` runs the identical
config via `pre-commit run`, on `push: branches: [main]` rather than
`pull_request`, since there are none to trigger on.

While wiring the local hook it turned out the hand-installed one on this
machine had drifted: it ran only `check-reviewed.sh` and `check-drift.sh`,
never picking up `check-layering.sh`, `check-sans-io.sh`,
`check-core-contract.sh`, or `check-unsafe.sh` as each landed — exactly the
"a gate nothing invokes is a preference too" warning `AGENTS.md` already
carries, just discovered concretely rather than left abstract. Updated the
local hook (untracked, machine-specific, superseded by this task's own
tracked config) to match while pip access to install the real tool is
unavailable here.

The CI half surfaced two adaptations the acceptance criterion's own wording
only gestures at:

- **The "rebase onto the previous commit" adaptation is not just for a future
  `origin/main...`-based gate — it is needed now.** Every current
  pre-commit-stage gate compares the **staged index against HEAD**, which
  means nothing after a push: there is no staged state, everything is already
  committed. The direct-to-main equivalent of "staged" is the pushed commit's
  own tree, and of "HEAD" is its parent — so the workflow does
  `git reset --soft HEAD~1` before running the pre-commit-stage gates (which
  leaves the last commit's changes staged against its parent, verified by
  running this exact sequence against a disposable clone of this repository's
  real history and confirming `git diff --cached --stat` showed the expected
  commit's files), then `git reset --hard "$GITHUB_SHA"` before the two
  commit-msg-stage gates, which read HEAD's own subject and its diff against
  its parent directly rather than the index. `fetch-depth: 2` is required for
  `HEAD~1` to resolve at all — the default shallow clone has no parent
  commit; a guard (`git rev-parse HEAD~1`) handles the repository's actual
  first commit, where no rebase target exists.
- **`check-reviewed.sh` cannot run in CI at all, and this is not a gap to
  close.** Its verdict lives in gitignored `target/review/`, which
  `.gitignore`'s own comment says is "consumed on the machine that made
  [it]" — deliberate, since the verdict gates *creating* the commit and is
  regenerable, not a durable record the way `reviews/` is. A fresh CI
  checkout never has it, for any commit, reviewed or not, so running the
  check there cannot distinguish "reviewed, but off-machine" from "never
  reviewed" — it would fail every push regardless of whether review actually
  happened. Per-commit review is architecturally a local, pre-commit-time-only
  gate: the commit cannot exist in history unless it already passed locally,
  under the honesty assumption non-negotiable 3 names as the one thing no
  script enforces. `SKIP: check-reviewed` in the CI job says why, not just
  that.

⚠️ **What could not be verified in this environment, disclosed rather than
silently assumed correct: the `pre-commit` tool itself.** No `pip` module is
available in this sandbox and no attempt was made to reach the network for
one, so `pre-commit install` and `pre-commit run` were never actually
executed here. What *was* verified directly: `.pre-commit-config.yaml` and
`.github/workflows/gates.yml` both parse as valid YAML (`python3 -c "import
yaml; yaml.safe_load(...)"`, catching a real defect this way — unquoted
`on:` parses as the boolean key `true` under PyYAML's default loader, a
known YAML 1.1 quirk GitHub's own parser special-cases but a generic one does
not; quoted to `"on":` and reverified); every hook `entry` script exists and
is executable; and every entry's actual command, run directly with the exact
arguments `pre-commit` would pass (no arguments for the six pre-commit-stage
hooks, one message-file path for the two commit-msg-stage hooks), behaves as
each gate's own extensive existing test history already established — this
task added no new check logic, only decided when the existing checks run.
The `pre-commit` framework's own dispatch is trusted the way this project
already trusts `git`, `bash`, and `python3` without re-verifying them from
first principles: a widely used, independently maintained tool, not
something built or owned here.

Review found two real defects, both reproduced directly rather than reasoned
about from the diff:

- **blocking**: `.pre-commit-config.yaml`'s own header claimed it wired
  "every gate script in scripts/," and it didn't — `build-index.sh --check`
  (M-1.33's own stated pre-commit gate for the generated index regions) was
  named nowhere in the new config, the CI workflow, or the pre-existing
  hand-installed local hook, so nothing anywhere would have caught a stale
  index once this task's hooks became the enforcement path. Fixed by adding
  it as a ninth hook (`build-index-check`, pre-commit stage, `entry:
  scripts/build-index.sh --check`) and to the local hook file.
- **major**: the CI workflow's reset sequence only ever examined the
  push's tip commit against its immediate parent — correct for a
  single-commit push, silently wrong for the common case here, since the
  `milestone` skill's own working style is many commits before one push.
  Reproduced by building a disposable clone, stacking a deliberately
  malformed commit (`wip: drop it, no task id, no trailer`) behind a clean
  tip commit, and confirming the original workflow's exact commands
  reported green — the malformed commit was never examined. Fixed by
  walking every commit in `github.event.before..github.sha` in order,
  checking each one out and running the same reset-and-check sequence
  against it individually, rather than only the range's final commit.
  `fetch-depth: 2` became `fetch-depth: 0` (full history) since a fixed
  shallow depth just moves the identical silent-miss failure to whatever
  batch size exceeds it, and how large a push can be is not bounded by
  design here. Reverified the exact malformed-commit scenario against the
  corrected loop: the buried commit is now caught (`gate_status=1`,
  correctly distinguishing it from the two clean commits surrounding it in
  the same simulated push).

Round 2 found one more real defect, more severe in practical terms than
either round-1 finding because it was not hypothetical: it is the literal
state of this repository's own pending history.

- **blocking**: the corrected multi-commit walk ran `pre-commit run`
  against every commit in the pushed range unconditionally — including
  commits older than the commit that introduces `.pre-commit-config.yaml`
  itself. Reproduced against real history, not a contrived scenario: this
  repository's actual `main` was, at review time, several commits ahead of
  `origin/main` (M-1.5 through M-1.37), none of which contain the new
  config file. Installing the real `pre-commit` tool and replaying the
  workflow's exact commands against that real range failed every one of
  those commits with `.pre-commit-config.yaml is not a file` — a false
  failure indistinguishable from a real one, and "essentially guaranteed to
  fire on the very first real push." The same problem generalizes past
  this one file: `scripts/check-commit-msg.sh` and
  `scripts/check-tests-kept.sh` themselves postdate several early
  commits in the walked range (they were introduced by M-1.6), so calling
  them unconditionally hits the identical class of false failure further
  back. Fixed with one general guard (`present_at`, `git cat-file -e
  "$commit:$path"`) applied to all three invocations, rather than three
  separate special cases — a commit is now checked only against what
  existed at that commit, matching `portability.md` rule 10's "a missing
  prerequisite skips with a named reason; it does not fail" for a tool,
  applied here to a file's own existence in history instead. Reverified
  against the real repository's actual pending range (31 commits, `before`
  set to the commit preceding this whole recent batch): the guard
  correctly identifies exactly one commit (this task's own) as having the
  config, correctly skips `check-tests-kept.sh` for the 25 commits that
  predate M-1.7, correctly skips `check-commit-msg.sh` for the 10 that
  predate M-1.6, runs every gate that does apply, and the loop exits 0 —
  every one of those commits is genuinely already-valid, already-`done`
  work. Reverified the round-1 buried-malformed-commit scenario once more
  on top of this fix to confirm no regression: still caught,
  `gate_status=1`.

Round 3, with network access this time to install the real `pre-commit`
tool, replayed the workflow's exact commands against this repository's
actual unpushed history (`origin/main` at `75bf42d`, local `HEAD` six
commits ahead) rather than a contrived one, and confirmed the concrete
case round 2 found broken now exits 0 exactly as intended, with no
regression on the round-1 scenario. **Pass, no findings.**

**M-1.33** closed a hole in progressive disclosure: skills had layer-1
descriptions and standards did not, so an agent could see *when* to load a skill
but had to open a whole standard or guess from its filename.

⚠️ It also produced a lesson about generated indexes. The first version replaced
the curated 39-entry tag index with 205 alphabetical raw tags — mechanically
correct and strictly worse, because the curated one is organised by *question*,
which is how people look things up. The split now is: **mechanical things are
generated (counts, tables, coverage), authored things stay authored**, and the
generated tag list is filtered to tags that group three or more documents.

**M-1.37** builds the outer loop. Doc 21 §8 designed it and then recorded why
it does not happen: it is "initiated by inspiration rather than by schedule" —
someone notices something and writes a milestone about it. ⚠️ **A phase that
runs when somebody thinks of it is not a phase**, which is the same argument
this project makes about every rule with no gate.

The inner loop reads one delta against one task, so it cannot see its own
drift: a convention quietly abandoned, two commits that each passed review and
contradict each other, or a spec that should not have been written that way all
pass it, because every individual step was faithful.

The artifact follows doc 21 §5's pattern but keys on **a set of commits** rather
than a diff, which is what makes reviews incremental and therefore
checkpointable: each names the commits it read, the gate unions them, and a new
commit is simply uncovered until some review covers it.

⚠️ **The step with teeth is `task_id`.** Every blocking or major finding must
name a backlog row that exists, or be argued. A cross-cutting finding recorded
only in `target/review/` is one nobody will act on — the directory is gitignored
and `cargo clean` reclaims it. That is what "findings become backlog tasks"
means operationally, and it is checkable.

⚠️ **What has no gate: the cadence.** Nothing bounds how many commits may
accumulate before a review, and no honest constant is available — it depends on
how much a reviewer can hold at once. Reviewing thirty commits in one pass
satisfies this gate and wastes it. ⚠️ **M-1 already has 24 commits and none has
been read as a whole**, which is the backlog of outer-loop work this task
creates rather than discharges.

Writing it produced one defect of the family M-1.9 catalogued, in the argued
escape: `grep -q … || argued=$?` only assigns when grep *fails*, so the variable
never became 0 and a finding argued in the baseline was still reported
unresolved. Exit-status plumbing written so the interesting branch cannot run.

Review returned `changes-requested` on three majors, and the sharpest is one
the *inner* loop produced.

⚠️ **The packet reported `check-milestone-review.sh: passed` for a milestone
whose every commit was unread.** `review.sh` discovers `scripts/check-*.sh`
automatically, so the new gate was picked up on the commit that added it — and
ran inside the materialised staged tree, whose `.git` M-1.9 deliberately plants
as an invalid file. No history, no milestone commits, and the gate's own
`skip … (no commits yet)` exit 0. Two independent defects composing into a
false pass: a **fail-open** in the gate, and the wrong **scope**. Both fixed —
the gate now hard-`fail`s when it cannot read history, and the gate is in
`GATES_EXCLUDED` because it is a *milestone completion* gate and a commit is
not required to have had its whole milestone re-read. ⚠️ Note which way the
composition ran: M-1.9's `.git` plant is what makes a git-using gate fail
closed, and it turned a gate that could not run into one reporting success,
because the gate answered "cannot run" with `skip`. **A gate that cannot run
must not report success**, and `skip` is a claim about applicability, not about
availability.

⚠️ **The documented escape was unreachable.** The skill and the packet both
said a blocking or major finding may be argued in `baselines/review.txt`
instead of becoming a task — but `record` refused to store a finding with no
`task_id`, and the `id` a baseline entry needs is assigned *by* `record`. So
the only path to the escape ran through the check that forbade it. It bit
hardest on `kind: spec-wrong` at blocking severity, precisely where the skill
says the outcome is a decision and not a task. Fixed by moving the demand from
`record` to the gate: record validates shape and prints the id, the gate
demands a resolution — the split `check-reviewed.sh` already used. ⚠️ The
general shape is worth keeping: **a documented escape nobody has walked is a
claim, not a feature**, the same argument this file makes about a gate whose
failure path nobody has run.

Two more, both about a check that answers with something other than what it
checked. `git show … | grep -q` on the baseline is a pipe whose left side dies
of SIGPIPE when grep exits at the first match — measured at rc=141 on a
200,000-line baseline, reporting a genuinely argued finding as unargued.
`check-reviewed.sh` escapes it only because its read carries `|| true`. And
`grep -qx "$tid"` treats a task id as a regular expression, so a hand-written
artifact claiming `task_id: ".*"` satisfies the one rule this gate calls the
one with teeth. `-F` closes it here; **the identical site in
`check-reviewed.sh` is M-1.38**, tracked rather than fixed in passing, because
it belongs to M-1.9.

**M-1.38** closes that tracked defect: `scripts/check-reviewed.sh:145` had
the identical `grep -qx "$task_id"` its sibling was fixed for, unescaped
against the backlog's known-id list. Reproducing before fixing, rather than
trusting the sibling's diagnosis to transfer unchanged, found the site
actually carried **two** bugs stacked on one line, not one: the regex
issue the backlog row names (`task_id: ".*"` in a hand-written artifact
matches every backlog row, satisfying non-negotiable 4 without naming a
real task), and the same pipe-form SIGPIPE misreport `check-milestone-review.sh`'s
own comment already describes for its sibling site — `printf '%s\n' "$known"
| grep -qx "$task_id"` rather than a here-string. Reproduced directly: a
~20,000-row `known` list with the real match on line 1 makes the pipe form
report "not found" for a task id that is, in fact, present, because `grep
-qx` exits at the first match, `printf`'s remaining write then SIGPIPEs, and
`pipefail` reports that early exit as the pipeline's failure. Both fixed in
one change — `grep -qxF "$task_id" <<< "$known"` — matching the sibling
site's exact resulting shape, since the two bugs share one line and one
`elif` and a fix for one without the other would leave the survivor
live. `tests/gates/negative.sh` gained a case for the regex direction (a
hand-written artifact with `task_id: ".*"` against a backlog with one real,
unrelated task), mutant-tested: reverting to plain `grep -qx` makes the
fixture wrongly pass. The SIGPIPE direction is real (reproduced above) but
not separately fixture-tested, the same size-dependent, environment-sensitive
shape `M-1.24`'s reachable-only-past-150x note and `check-milestone-review.sh`'s
own eighth-round finding already describe for this exact bug class — a
~20,000-line fixture is disproportionate to check into a suite that runs on
every commit.

A second round found two more blocking, and both are the same mistake in
different clothes: **a check written against the example in front of it.**

⚠️ **The milestone derivation matched exactly one milestone in the project's
life.** `^## (M-[0-9]+):` requires the hyphen, and only *this* milestone has
one — the roadmap's next headings are `## M0 —`, `## M1 —`, `## M8 —`. From M0
onward the gate would print "no milestone is decomposed" and exit 0: a false
statement, and a pass for a milestone whose every commit was unread. Once
M-1.16 wires it into the completion condition, every remaining milestone in the
roadmap could be declared complete with no cross-cutting review at all. ⚠️ It
also fails **quietly and late** — nothing would have gone wrong until the first
commit after M-1, months from the code that caused it. `known_task_ids` and
`check-commit-msg.sh` both already use `M-?[0-9]+`; this was the only place
that assumed otherwise, which is the tell: a project-wide id shape re-derived
locally, from the one example the author could see.

⚠️ **Rule 2 wedged the gate shut on `git commit --amend`.** git.md rule 21
endorses amending freely before a push, and this project never pushes unasked,
so every commit is amendable. A message typo fixed after a review left an
artifact naming a SHA that history no longer reaches — and rule 2 called that
"a review claims a commit the milestone does not contain", forever. The argued
baseline resolves *findings*, not coverage, so the only escape was hand-deleting
a file in gitignored `target/review/` that no message named, which also
discarded the coverage of every other commit in the milestone: a typo fix
costing a full re-review. The gate also printed its `FAIL` and then `ok … all
commits covered`, two contradictory lines with the reassuring one last. Now a
commit HEAD no longer reaches is a superseded artifact (`warn`), a commit that
is reachable but belongs elsewhere is still a failure, and the failure path
`finish`es before it can contradict itself. ⚠️ The first attempt at this used
`git cat-file -e`, which succeeds for an amended-away commit because the object
survives in the reflog — **reachability, not existence, is the question**.

And the packet's `## What changed across them` used `${first}~1`, which the
**root commit does not have**. M-1.0 is this repository's root *and* its first
unreviewed commit, so the first intended use of this tool hit it: `2>/dev/null`
swallowed the fatal and the fallback showed the root commit's stat alone — 73
files of research corpus, none of the work under review — under a heading
claiming it was the shape of all 24. The empty tree is the correct base.

⚠️ Three of these five were **fail-open in a gate whose subject is fail-open**,
which is worth stating plainly rather than filing away: writing the check does
not confer the property, and the only thing that found them was running them.

A third round found the *same* fail-open a third time, one layer further in.
Fixing the hyphen made `grep -m1 -oE '^## M-?[0-9]+'` match M0 — but line 13 of
this file says **"Completed tasks stay here with their commit reference"**, so a
finished milestone keeps its section and `-m1` returns M-1 forever. From M0
onward both scripts would have kept checking a fully covered M-1 and reported
success while the milestone actually being built went unread. ⚠️ **Twice in a
row the milestone derivation was written against the only example available**,
which is the argument for deriving it from something that cannot be one example:
the current milestone is now the one **HEAD is working in**, read from the most
recent commit subject that names a task. This gate's subject is commits, so
deriving from commits needs no convention, and it flips to M0 exactly when M0's
first commit lands — not before, which is what leaves M-1's completion still
checkable after M-1's last commit.

Two honesty fixes came with it. The `ok` line said "all N commit(s) covered"
when what it counted was commits whose subject *begins* with a task id of this
milestone — so the shapes `check-commit-msg.sh` deliberately exempts, `Revert
"…"` and merges, are never required to be covered. It now says what it counted.
⚠️ **Closing that gap needs a decision about what a revert's coverage means**,
and is not this task. And the unresolved-finding failure named neither the
baseline nor the requirement that the argued line be *staged*, so an operator
taking the documented escape got an unchanged failure and repeated the edit;
`check-reviewed.sh` already had both aids.

A fourth round found the artifact in the wrong place. ⚠️ **Milestone coverage
was stored in gitignored `target/review/`**, so `cargo clean` erased the record
that a milestone had been reviewed — and M-1.16's completion condition calls
this gate while M-1.14 puts gates in CI, which means the condition would have
passed on exactly one machine and been permanently red on every clone. The
per-commit reviewer is right to write there: its verdict is keyed to a staged
diff about to become a commit, and it is consumed where it was made. A
milestone's coverage is the opposite kind of claim — durable, about history,
and something a second agent has to be able to check. It now lives in tracked
`reviews/`, read **from the index** for the reason `baselines/review.txt` is:
a file that counts while unstaged rewards the path that leaves no trace.

⚠️ That move exposed a regress the on-disk version had hidden: **committing a
milestone's verdict creates a commit that names a task in that milestone**, so
the milestone goes from fully covered to one-uncovered the instant it is
recorded, and covering that commit needs another review in another commit.
Measured, then fixed by excluding commits whose every changed path is under
`reviews/` — narrowly, so a commit carrying real work alongside a review file is
still subject matter.

The rest of the round was duplication and honesty. `current_milestone` and the
commit-enumeration grep were **byte-identical copies** in the driver and the
gate, and the two rounds above had each had to land in both; they are one
definition in `lib.sh` now, because if the gate ever enumerates a commit the
driver did not show the reviewer, `record` refuses the SHA the gate demands and
the gate cannot be passed at all. `record` called an abbreviated SHA "not a
commit in M-1" — false, and pointing at the wrong problem, while this tool's own
`coverage` output prints exactly that abbreviated form; an unambiguous prefix is
resolved now. And an absent roadmap section rendered as an empty heading, which
reads as "this milestone had no goals"; it says so instead, on both streams.

⚠️ **A fifth round found the SIGPIPE bug again, in the code written to fix the
regress above, in the same commit that documents fixing it elsewhere.**
`printf '%s\n' "$paths" | grep -qv '^reviews/'`: `grep -qv` exits at the first
non-matching line, `printf` dies of EPIPE, `pipefail` calls the pipeline failed,
and the leading `!` inverts that to true — so a commit whose changed-path list
overflows the pipe buffer was classified as review bookkeeping and **dropped
from the milestone entirely**, with the gate then reporting full coverage
without it. Measured: correct at 1000 changed files, wrong at 1500 (~57 KB);
with a 204 KB path list the commit vanished and the fixed version keeps it.
⚠️ It is exactly the **largest** commits that disappear — generated protocol
code, an imported corpus, a mass rename — and the same shared helper makes
`record` refuse to store a verdict naming one, so a reviewer who did read it
could not record having read it. A here-string fixes it. That this class has now
appeared **nine times across six scripts**, including inside its own fix and its
own documentation, says it is not carelessness but a property of the idiom: `set
-o pipefail` makes every `cmd | grep -q` a place where a *successful early exit*
is reported as failure. It belongs in the shell rules `portability.md` will grow.

Two smaller ones. A `task_id` the backlog lists but marks **done** discharged a
blocking finding — plausible to write, since the row that introduced the code is
often the one a finding is about, and fatal in effect: `next-task` reads `todo`,
so the finding is parked where nothing will look again, which is the exact state
rule 3 exists to prevent. The row must now still be open. And ⚠️ `known_task_ids`
reads the backlog from the **working tree** while artifacts and the argued
baseline are read from the index, so an unstaged row satisfies the gate locally
and fails it on CI — the same pass-here/red-there asymmetry this task fixed for
the artifact location. It is shared by three gates and is **M-1.39**, not
M-1.37's to change.

**M-1.39** closes that shared defect. `known_task_ids` and `open_task_ids`
both read `docs/internal/product/backlog.md` as a plain path on disk; fixed
by a new shared helper, `_backlog_from_index`, that reads
`git show ":docs/internal/product/backlog.md"` instead — the exact read
`check-reviewed.sh`'s own `baseline_staged()` already uses for
`baselines/review.txt`, for the identical reason. `git show ":path"` reads
stage 0 of the index regardless of a later, unstaged working-tree edit,
confirmed directly: staging a backlog with one row, then appending a second
row to the file *without* staging it, `known_task_ids` still reports only
the first — and staging the second row makes it appear, with no commit
required either time (the index, not `HEAD`, is what `:path` reads). A
repository with no `backlog.md` staged at all — `git show` exits 128 with a
`fatal:` line — degrades the same way the old `[[ -f "$backlog" ]]` guard
did, silently, via the same `2>/dev/null || true` this file's other
Python-backed gates use for a different kind of expected failure.

One shared helper rather than the read duplicated in both functions,
since they already live in the same file — unlike `check-readmes.sh`'s
TOML parser, which is a second copy of `check-layering.sh`'s across
*different* files with no shared Python module to hold a common one, this
is one file, one function, one place to fix if the read ever changes again.
Switching from `grep ... "$file"` to `grep ... <<< "$var"` for both
functions' first `grep` also converts them to the here-string form `M-1.38`
just established as the safe idiom, rather than reintroducing a
`printf | grep` SIGPIPE risk while touching the exact code this milestone
has now fixed that bug in twice.

A new `tests/gates/negative.sh` case (`check-commit-msg.sh`, an unstaged
backlog row) proves the fix rather than only the header's prose: a backlog
staged with one real task, then a second row appended to the working tree
*without* staging it, and a commit subject naming that second, unstaged
task. Mutant-tested: reverting `known_task_ids` to the old working-tree
read makes the fixture wrongly pass — the local-pass/CI-fail asymmetry this
task exists to close, reproduced and then closed in the same fixture.

Review found that fixture exercises only `known_task_ids`'s half of the fix
— reverting `open_task_ids` alone, leaving `known_task_ids` fixed, was
checked by hand to leave the entire `tests/gates/negative.sh` suite still
green. `open_task_ids` is verified correct by direct reproduction (both
mine and, independently, the reviewer's own mutant test), just not
regression-guarded the way `known_task_ids` now is. Not given its own
fixture here: `open_task_ids`'s only caller is `milestone-review.sh
record`, a driver `tests/gates/negative.sh` does not currently exercise at
all — its counterpart gate, `check-milestone-review.sh`, already has a
case, but `record` itself needs a verdict JSON naming real commits against
a real milestone, the same setup weight `check-milestone-review.sh`'s own
fixture already carries, and duplicating that machinery for one function's
regression coverage is a larger addition than this task's own scope.
Accepted as a documented, non-blocking gap per `review.md` rule 14, the
same call made for `check-portability.sh`'s round 3 (a fix verified
correct but outside what the existing suite's shape can cheaply cover) —
a candidate for a future task if `milestone-review.sh record` itself ever
gets brought under `tests/gates/negative.sh`'s framework.

**M-1.44** writes down the class M-1.9's sixth round named ("it belongs in
the shell rules `portability.md` will grow," above) rather than leaving it
tribal knowledge scattered across commit messages and this file's own
prose. A new "Shell scripting" section, appended after rule 20 rather than
inserted earlier in the file: rule 2 is already cited by number from
`lib.sh`, and rule 10 from `check-milestone-review.sh`, so renumbering
anything before them would break those live citations — the exact kind of
drift this whole file exists to prevent, avoided by construction instead of
caught later. ⚠️ Corrected by review: an earlier draft of this paragraph
said both scripts cited both rules; `check-milestone-review.sh`'s only other
"rule 2" is its own local numbered checklist ("# 2. no artifact claims a
commit the milestone does not contain"), not a `portability.md` citation,
and not itself evidence either way. Two rules: 21 names the mechanism
precisely (`grep -q`/`head` exit
early, the pipe's other end SIGPIPEs on its next write, `pipefail` reports
the *consumer's success* as the pipeline's failure — and why it passed
every one of the nine times it shipped: the producer has to be large
enough to still be writing when the pipe closes, which a small test
fixture rarely is until it happens to be), and 22 gives the two remedies
already in use throughout this milestone's gates (a here-string when the
data is already in a variable, `\|\| rc=$?` when the producer must run as
a real command). Acceptance also asked for a citation from an actual gate
script, not just the standard existing in isolation: `check-reviewed.sh`'s
own `M-1.38` comment — the freshest, most directly relevant instance of the
idiom in the tree — now points to rules 21–22 by name, so a future reader
hitting that comment finds the general rule rather than re-deriving it from
one site's specific reasoning.

**M-1.48** closes the seventh site: `check-commit-msg.sh:133` had the
identical `printf '%s\n' "$known" | grep -qx "$id"`, missed when `M-1.38`
fixed the sibling in `check-reviewed.sh` and `M-1.37` fixed it in
`check-milestone-review.sh`, and outside `M-1.44`'s own list of six because
it was found later, by `M-1.38`'s own review. The regex-injection half of
the sibling fixes does not apply here — `$id` can only be something the
subject-format regex earlier in the same script already matched
(`M-?[0-9]+\.[0-9]+`), never a hand-written string with a metacharacter —
so `-F` is defense in depth rather than a live bug at this site; the
SIGPIPE-misreport half is the real one, and `M-1.44`'s "Shell scripting"
section rules 21-22 are now cited directly from the fixed line's own
comment. Reproduced both directions before and after the fix, in isolation
rather than through the full gate invocation: run through
`check-commit-msg.sh` itself against a real ~20,000-row backlog, the race
did not trigger on the first attempt (SIGPIPE reproduction is inherently
timing-dependent, not deterministic) — so the exact vulnerable line was
extracted and run standalone several times instead, where the pre-fix
`printf | grep -qx` form misreported a real, listed task id as unlisted on
every attempt, and the fixed `grep -qxF ... <<<` form was correct on every
attempt. A construction that reproduces unreliably through one invocation
path but reliably in isolation is still a real defect; the fix removes the
open pipe the SIGPIPE needs entirely, which is what makes it not merely
"less likely to fail" but structurally unable to fail this way.

**M-1.31** writes the `contract-change` skill the "Skills" group's own
Remaining column has named since the milestone was decomposed: the procedure
for the one thing `AGENTS.md` non-negotiable 6 and `contracts.md` rules 12–15
require atomically — a `pub trait`'s method set, every fake beside it, every
real implementation, every forced call site, and the ADR, in one commit.
Unlike most of this milestone's remaining tasks, every gate the skill points
at already exists and passes (`check-core-contract.sh` landed at M-1.10,
`check-layering.sh` and `check-sans-io.sh` at M-1.8), so the skill is not
written against a promise the way `contract-change` itself would have had to
be a few tasks earlier — it is written against scripts already exercised in
this milestone's own commits, including `check-core-contract.sh`'s own
description of what it deliberately does not check (an implementor merely
listed in the diff without being genuinely updated, a call site's body
matching the new signature), which becomes this skill's own "before
committing → review" step rather than a gap left unnamed. Follows the same
frontmatter and adapter shape as every other skill: `name`/`description`
only, a `.claude/commands/contract-change.md` pointer, and both skill-index
tables updated — `AGENTS.md`'s via `scripts/build-index.sh` (generated, not
hand-edited — the marker comment says so and hand-editing it would just be
overwritten the next run), `.agents/skills/README.md`'s by hand since that
table is authored, not generated.

**M-1.45** backports the crash-safety wrapper `build-index.sh` established
and `check-unsafe.sh` reused to the two scripts that never adopted it,
`check-layering.sh` and `check-core-contract.sh`: the Python body's top-level
code moved into a `build()` function, called from a `try`/`except Exception`
that maps an uncaught crash to exit 3 and a `CRASH ...` line, distinguishing
it on the bash side from both a clean pass and genuine violations found. Both
scripts already avoided a false pass today, but only because their one risky
call (`package_name`/`runtime_deps`'s `read_text()`, `extract_impl_files_by_trait`'s
`git show`) happens to run before either script's first `print` — an
accident of statement order, not a property either script guaranteed, and
exactly what the backlog row's own acceptance criterion named. Verified in
scratch fixtures, not a permanent `tests/gates/negative.sh` case (the same
verification level `check-portability.sh`'s crash path got at M-1.30): a
clean tree still reports `ok`, a genuine violation still reports `fail`, and
a contrived non-UTF-8 byte in a tracked file now reports the crash
explicitly instead of dying silently. The false-pass class itself was then
reproduced directly, not merely reasoned about: a mutant of
`check-core-contract.sh` with the wrapper stripped back out *and* the
implementor scan reordered to run after the first `CHANGED` print — a
plausible future refactor, not a contrived shape — reported `ok trait Clock
changed with every implementor and an ADR in this commit` against the exact
fixture that crashed, while the real fixed script correctly reported the
crash on the same input. That confirms the wrapper is load-bearing against a
danger that does not yet exist in either script's current statement order,
not decorative insurance against a danger that was never real.

Review found that this verification level — scratch fixtures, not a
checked-in gate — is exactly what the task's own acceptance criterion cited
as precedent ("the way `check-unsafe.sh`'s own suite already does") without
actually being true: no crash-path case exists in `tests/gates/negative.sh`
for any script, `check-unsafe.sh` included. Not fixed here — out of this
task's scope — tracked as **M-1.51**.

**M-1.46** gives `m-1-complete.sh` (M-1.16) the permanent `tests/gates/negative.sh`
coverage M-1.15 established as the standard one commit before M-1.16 landed,
and which M-1.16's own commit message and retrospective admit it never got —
five ad hoc scratch repos, run once by hand, not preserved. Two cases, not
one, because the acceptance criterion names two distinct failure classes:
`AGENTS.md` missing its `## Non-negotiables` section exercises the ordinary
"problems found" path (the parser's `if not m: ... sys.exit(2)` guard), and a
non-UTF-8 byte in `AGENTS.md` exercises the crash-safety wrapper this script
has had since M-1.16 itself — distinct from the first case, since the read
that raises `UnicodeDecodeError` happens before the regex search ever runs,
and confirmed distinct by inspecting the captured output directly rather
than trusting the exit code alone: the first case prints `PROBLEM AGENTS.md
has no ## Non-negotiables section` (exit 2), the second `PROBLEM the
AGENTS.md parser raised UnicodeDecodeError: ...` (exit 3) — the same
distinction `check-unsafe.sh`'s and `check-core-contract.sh`'s own crash
paths draw between "a real violation" and "the scanner died."

`copy_gate`'s `<script-name>` argument turned out to already handle
`m-1-complete.sh`'s nested location (`scripts/gates/`, not `scripts/`)
without changes — passing `gates/m-1-complete.sh` copies the right file to
the right place, since the helper only ever joins its two arguments as a
path — with one addition: `new_scratch` only creates `$dir/scripts`, so each
setup function makes `$dir/scripts/gates` itself before calling `copy_gate`.

Both fixtures are deliberately minimal — `AGENTS.md` plus the one gate
script, nothing else `m-1-complete.sh` would need to reach a genuine `ok`
(every non-negotiable's own gate script, `tests/gates/negative.sh`,
`check-milestone-review.sh`). That is structural, not a shortcut: unlike
every other gate this suite tests, `m-1-complete.sh`'s success path needs
the *entire* repository correctly in place, which is a fixture size no other
case here approaches and which the task's own acceptance criterion does not
ask for — only that a broken artifact makes it fail. Verified this
minimalism does not accidentally widen what each fixture actually tests: a
control run with a well-formed `## Non-negotiables` section (a valid rule, a
following `## ` header) gets past the parsing step cleanly and fails only
several steps later, for the unrelated and expected reason that the
downstream scripts a real repo would have aren't in the scratch fixture —
confirming both real fixtures' failures are attributable to the specific
defect each is named for, not to the fixture being incomplete in some other
way.

**M-1.51** closes the gap `M-1.45`'s own review found: neither
`check-layering.sh` nor `check-core-contract.sh` had a checked-in crash-path
case, despite the acceptance criterion citing `check-unsafe.sh`'s suite as
having one — which turned out not to be true either, so this closes the
first two of what could be read as a wider gap without inventing new
scaffolding to do it. Both new cases reuse the existing `setup_layering`/
`setup_core_contract` fixtures' own shape almost unchanged, with the one
risky call each script makes fed a non-UTF-8 byte instead of valid input:
`check-layering.sh`'s case corrupts the dependent crate's own `Cargo.toml`
(`package_name()`'s `read_text()`), `check-core-contract.sh`'s case adds a
second, unrelated tracked `.rs` file with the bad byte alongside a genuine
trait-method-set change (`extract_impl_files_by_trait()`'s `git show`,
which scans every tracked `.rs` file, not only ones mentioning the changed
trait). Named `(non-UTF-8 crash)`, the same second-case-against-one-gate
convention `check-reviewed.sh (regex task_id)` and `check-hot-path-bench.sh
(required row)`/`(leftover entry)` already established. Verified beyond the
suite's own exit-code check: each fixture was run directly outside
`tests/gates/negative.sh` and its captured output inspected, confirming
`FAIL the layering scanner crashed`/`FAIL the core-contract scanner
crashed` and a `CRASH ...` line in both cases — the crash-wrapper path
specifically, not a coincidentally-nonzero exit from some other cause.

**M-1.37**'s fourth run of the outer loop (`d469d32`) is the boundary
review: every M-1 task besides the documented, blocked `M-1.12` had just
landed when it was dispatched. `changes-requested`, two major findings, each
independently verified against the current repo state before being recorded
(not merely trusted from the milestone-reviewer subagent's report) —
`milestones/M-1.md`'s "Notes for the boundary review" had drifted stale a
fourth time, the exact pattern checkpoints 1–3 already named and tried to
fix twice (→ **M-1.52**), and `check-unsafe.sh`'s crash-safety wrapper had
never been exercised by any test despite being acknowledged twice without a
tracking row (→ **M-1.53**). Both findings' backlog rows were written and
staged before the finding could be recorded, per the skill's own rule that a
cited task must already exist; the milestone-reviewer subagent was then
handed the row IDs and asked to record against them rather than guess.

**M-1.52** closes the fourth recurrence by changing what recurred, not just
its symptom: the prior two fixes (`M-1.47`, `M-1.49`) each refreshed a
hand-written list of open task IDs, and each refresh went stale the moment
the next task closed or opened. This time the list is removed
outright. `milestones/M-1.md`'s closing "For the boundary review" paragraph
no longer names any task ID; it tells the reader to ask `backlog.md`
directly, which cannot go stale because it says nothing that changes.
`roadmap.md`'s task-count cell drops its number the same way, replaced with
`see backlog.md ⚠️` — the identical move `milestones/M-1.md`'s own Tasks
section header already made for the same reason. The one exception is
deliberate, not an oversight: each checkpoint's own numbered findings list
(`Checkpoint 1` through `Checkpoint 4`) still names the specific tasks it
produced, because that is a historical record of what a specific checkpoint
found, not a live claim about what is currently open — the distinction the
new closing paragraph draws explicitly, so a future editor does not "fix"
history into staleness again by trying to keep it current. Also corrected in
the same edit, the identical class of staleness this finding names: the
Tasks table's `contract-change (M-1.31)` moved from "Remaining" to
"Landed," and the "44 tasks" bullet in "Risks and open questions" — stale
since well before this checkpoint, found only because fixing the adjacent
paragraph meant rereading this one — now points at `backlog.md`'s row count
instead of carrying its own number.

⚠️ **M-1.53's own acceptance criterion turned out not to be satisfiable as
written, and the deviation is recorded here rather than forced.** It asked
for "the same shape" as `M-1.51` — a non-UTF-8 byte in a tracked file,
verified to produce a `CRASH ...` line. Probed directly before writing any
fixture, in three separate scratch repos, none of them crashed
`check-unsafe.sh`: a non-UTF-8 byte in a tracked `.rs` file's *content*
(caught by the per-file `except (OSError, UnicodeDecodeError)` guard,
reported as a `warn` and skipped), in the file's *name* (git quotes an
unrepresentable filename, the quoted string then resolves to no real path,
so `open()` raises `FileNotFoundError` — still `OSError`, still caught), and
in `baselines/unsafe.txt` itself (read via bash `git show` into an env var,
decoded by Python's `os.environ` with `surrogateescape`, which does not
raise). This is not a defect: `check-unsafe.sh` was already more defensive
than `check-layering.sh`/`check-core-contract.sh` were before `M-1.45` —
which is exactly why a bare non-UTF-8 byte crashed *them* and does not crash
this one — and no small, realistic fixture was found that reaches its outer
`try`/`except Exception` wrapper at all.

Rather than force a synthetic fixture built to exploit an unknown, unproven
bug — the opposite of this project's own testing philosophy, which tests
understood failure modes, not invented ones — the honest, narrower thing was
implemented instead: `(non-UTF-8 file doesn't suppress a real violation)`, a
fixture combining an unreadable file with a genuine violation elsewhere,
proving the `warn`-and-skip on the first does not stop the scan before it
reaches the second. Checked against `git ls-files`'s actual return order,
not assumed, since an ordering where the real violation sorts first would
let the case pass by accident regardless of whether a skip really continues
the scan — the unreadable file's path was chosen to sort first. Mutant-tested
by replacing the per-file loop's `continue` statements with `break` in a
scratch copy: the mutant reports the tree `ok` (0 files scanned) against the
identical fixture that makes the real script report `fail` — confirming the
fixture is load-bearing for the property it actually tests, even though
that property is not the literal crash wrapper the acceptance criterion
named. Flagged here for review to weigh in on, per the precedent `M-1.29`
set for exactly this shape of literal-criterion-versus-reality tension.

⚠️ **A sixth round found that round five's own fix punished the honest path.**
Requiring a cited backlog row to still be `todo` was checked by the gate on
every run — so the moment the finding's task was implemented and its row ticked
to `done`, the gate went red and stayed red, because the artifact is cumulative
and never re-read. The three ways out were re-opening a finished row (a lie),
arguing a finding the author had actually acted on, and re-recording the verdict
without the finding — **deleting a finding to make a check pass**, non-negotiable
2 exactly. The rule itself is right; its place was wrong. Openness is an
authoring-time question, enforced by `record` while the author is still there to
pick another row, and the gate is back to what the acceptance criterion says:
the task exists. ⚠️ Third time in this task that a check written to close a hole
made the correct behaviour the expensive one — worth naming as a review question
in its own right: *what does this gate cost someone who is doing the right
thing?*

The anti-regress carve-out was also too wide. Excluding every commit whose paths
are all under `reviews/` dropped a commit that only rewrote `reviews/README.md`
— real work, silently unreviewed, gate green. Only the verdict files cause the
regress, so it matches the artifact name now.

And ⚠️ **nothing said who runs a milestone review.** The inner loop is explicit
that a change is reviewed by an agent that did not write it, and the outer loop
had no equivalent — so as wired, an agent could drive a milestone to its last
commit and then review its own 24 commits, which is the one reader guaranteed
not to see their drift. Now stated in the packet, in both skills, and given an
adapter in `.claude/agents/milestone-reviewer.md` so it is a thing to do rather
than a thing to remember. ⚠️ It is still unenforceable — the artifact records a
`reviewer` string nothing can verify — and that is said where the other
unenforceable claims are said, rather than implied away.

⚠️ **A seventh round found that round six's edit had deleted the baseline read.**
Removing the gate-side openness check took `baseline_text="$(git show …)"` with
it — adjacent lines, one edit — leaving the argue branch referencing a variable
assigned nowhere. Under `set -u` that aborts the condition, `argued` stays 1, and
**the entire argue escape was dead code**: an operator who did exactly what the
gate's own remedy line told them to got the identical failure back, with a raw
`baseline_text: unbound variable` on stderr naming no gate. It failed hardest for
`kind: spec-wrong` at blocking severity — the one case the skill says must *not*
become a backlog row — so a milestone carrying one could never be completed.

⚠️ Two lessons, and the second is the one worth keeping. First: a removal is a
change, and this one was never run. Second and larger — **six of the seven
rounds found a defect on a path that had been described but never walked.** The
escape was documented in the skill, in the packet, and in the gate's remedy
note, and the documentation was written before anyone tried it. This is the
concrete form of non-negotiable 3 for gates rather than tests: *a failure path
nobody has walked is a claim, not a feature*, and it belongs in whatever
`sdd.md` grows for acceptance criteria. The fix here was one line; finding it
took walking the two documented steps once.

⚠️ **An eighth round, run after actually walking that same escape end to end,
returned `pass`** with one minor: `known_task_ids | grep -qxF` was the one site
in this file the earlier sweep for this exact SIGPIPE class missed — reachable
only past roughly 150x today's backlog size, so not urgent, but the same class
this task spent seven rounds eliminating and one line to close. Fixed on sight
rather than filed, since the file was already open.

**M-1.36** renames the `goal` skill because the name is one a tool is likely to
claim. ⚠️ The collision would be **silent**: a slash command resolves to one
procedure, and nothing announces that the other exists. `milestone` says what
the skill drives and is far less likely to be taken. Nothing about the loop
changed — only the name, the directory, the adapter, and the index entries.

⚠️ The general rule this suggests, and which no gate holds: **a skill's name is
part of its interface with the tool, not only with the reader.** `spec`, `tdd`,
`adr`, `research`, and `review` are all short enough to be claimed the same way.
M-1.30's `check-portability.sh` is the natural home for a check, if one is worth
having.

**M-1.30** is `check-portability.sh`, `.agents/skills/README.md`'s "The rules"
1–3. Three checks, each a direct translation of one of those rules rather than
anything invented: no line in `AGENTS.md` or any `SKILL.md` starts with `@` (the
one vendor syntax this repository actually names by example — `CLAUDE.md`'s own
`@AGENTS.md`/`@.agents/skills/README.md`), every `SKILL.md`'s frontmatter has a
non-empty `name` and `description` with `name` matching its own directory, and
every `.claude/commands/*.md` both points at a skill that exists and contains
neither a markdown heading nor a numbered step — the shapes a real procedure
would have and the eight actual adapter files, checked by hand first, do not.

Deliberately left out: the name-collision risk `M-1.36`'s retrospective raised
above ("a skill's name is part of its interface with the tool"). Checking it
would need an authoritative list of names some other tool might claim, which
does not exist here to check against — the same reason `requirements.md` marks
NFR-55/56 UNDERIVED rather than guessing a number. `M-1.36`'s own note already
says this explicitly ("if one is worth having"); inventing the list now would be
exactly the kind of unsourced number this project's standards forbid.

Fenced code blocks are stripped before the vendor-syntax scan, confirmed
necessary by testing rather than assumed: a skill illustrating `@import` as a
*counter*-example inside a ` ``` ` block should not itself be flagged, even
though nothing in this repository currently does that. Verified end to end in
scratch fixtures for every branch — a vendor-syntax line (and its fenced-block
exemption), a `SKILL.md` name/directory mismatch, a missing `description` on
both `SKILL.md` and a command file, a command file shaped like a procedure
(heading, numbered step), and a command file pointing at a skill that does not
exist — plus the crash-safety wrapper this session's other Python-backed gates
already use, exercised directly with a non-UTF-8 `AGENTS.md`. One
`tests/gates/negative.sh` case (the vendor-syntax line, the rule this gate's
own standard names most concretely) was added and mutant-tested: with the
check's `if` condition disabled, the fixture wrongly passes.

Review found two real defects the "verified every branch" list above did not
actually cover, both in how fence-stripping was applied rather than in
whether it was justified. First, `enumerate()` numbered the vendor-syntax scan
*after* fenced lines were already dropped, so a violation reported on a file
with a fence earlier in it got the wrong line number — reproduced by
appending a violation to the true end of `review/SKILL.md` (already 114 lines
with two fenced blocks): the gate reported line 110, not 115. Fixed by
carrying each surviving line's original position through the strip rather
than renumbering what remained. Second, the command-file heading/numbered-step
scan never stripped fences at all, despite the header's own stated reason for
stripping them elsewhere — a pointer file legitimately quoting a bad example
inside a fence would be flagged as "looks like a procedure" for its own
counter-example. None of the eight real command files trigger this today, but
review reproduced it directly in a scratch fixture. Fixed by routing that
scan through the same fence-stripping helper the vendor-syntax check uses,
now shared rather than duplicated. Both fixes reproduced exactly as review
described, then reproduced as fixed, before re-review: the corrected line
number (115, not 110) against the real `review/SKILL.md`, and a scratch
fixture with a fenced bad-example block passing cleanly where it previously
failed — with a genuine, unfenced procedure-shaped command file confirmed to
still fail, so the fix narrows the exemption rather than widening it into a
loophole.

Round 2 review, verifying the round-1 fixes rather than the original diff,
passed both reproductions but found a third defect in the same shared
helper: a ` ``` ` marker with no closing partner left `in_fence` `True` for
the rest of the file, so `strip_fenced_lines` silently dropped -- exempted
from every scan -- every line from the unclosed marker to end of file,
including a real violation. Not currently triggered by any file in the
repository (fence-marker parity checked across all 17), but an ordinary
missing-closing-fence typo away from disabling this gate's remaining
coverage of a file with no diagnostic at all. Fixed by counting `` ``` ``
markers before stripping: an odd count grants no fence exemption for that
file at all (fails closed on the whole file rather than open on the
remainder) and is itself reported as its own problem, the same "cannot skip
and still mean anything" discipline `lib.sh`'s `require_sha256` and
`require_python` already use. Reproduced review's exact construction against
both a `SKILL.md` and a command file, confirmed failing after the fix where
it silently passed before, and a second `tests/gates/negative.sh` case
covers it, mutant-tested the same way as the others: with the odd-count
check disabled, the fixture wrongly passes.

Round 3 passed, with one minor finding recorded but not required to be
fixed before this commit, per `review.md` rule 14: round 1's two fixes (line
numbers surviving fence-stripping; fence-stripping applied to the
command-file scan) have no dedicated regression case of their own, unlike
round 2's fix. Independently re-verified correct in round 3's own
reproduction, so nothing is wrong today — but `tests/gates/negative.sh`
only asserts on exit code, never message content (no case in the suite
does), so a case genuinely checking "the line number is 115, not 110" does
not fit the suite's current design without extending it — a bigger change
than this task's own scope, and left as a noted gap rather than invented on
the spot.

**M-1.35** is a defect M-1.9's work found in an already-`done` gate, and its
review is a warning about fixing diagnostics. The first fix made the gate speak
where it had been silent — and said something **false** in two cases: it ran the
comment-stripping branch in `HEAD` mode, where there is no message file, and it
stripped comments but **not** leading blank lines, which git also removes — so
on a `COMMIT_EDITMSG` whose first line is empty it returned that blank line as
the subject. Both produced "the commit message has no subject" for a commit
whose subject was plainly there. ⚠️ A wrong
diagnosis is worse than the silence it replaced, because it sends the author to
fix something that is not broken.

⚠️ The second fix then introduced a **regression the original did not have**.
`git commit` with an editor uses `cleanup=strip`, which removes comment lines;
`-m` and `-F` use `cleanup=whitespace`, which keeps them. So for
`git commit -m '#42 note' -m 'M-1.35: …'` the subject that lands is `#42 note`,
and a script that skips comment lines reads the *body*, reports
`ok … names M-1.35`, and admits a commit naming no task — the one thing this
gate exists to refuse. No script can know which cleanup mode git will apply, so
the ambiguity is now the failure: the subject is the first non-blank line, and a
comment there is refused rather than searched past. That also stops the
`commit.verbose` case reporting `got: diff --git a/f b/f`, a subject nobody
wrote.

**M-1.34** exists because doc 21 §4 says the reviewer receives "the relevant
standards", and until now that phrase had no referent. Handing a reviewer all
eight dilutes its attention across seven that do not apply — the same failure
§4 is trying to avoid by withholding the author's reasoning.

The mapping lives in each standard's own front matter rather than in a table
inside the script, for the reason M-1.33 established: two places holding one
fact is a promise to keep them in sync forever. ⚠️ Most patterns name Rust that
does not exist yet, so they are **unexercised until M0** — deliberate, but it
means the mapping is asserted rather than demonstrated for every crate-scoped
entry.

⚠️ The first independent review of this task found a defect the author's own
testing had not: `git diff --cached --name-only --diff-filter=ACMR` silently
drops deletions, so a **deletion-only change reported "nothing staged" and
routed to no standards at all** — including `git.md`, whose `*` is meant to be
unconditional. Deleting a test or a standard is exactly the commit class
non-negotiable 2 exists for, and it was the one class the router could not see.
The review also found `applies_to` claimed too few crates in two standards:
`security.md` did not claim `oqueue-buf` or `oqueue-checksum`, which are two of
the three crates where `unsafe` is allowed, and `performance.md` did not claim
`oqueue-broker`, `oqueue-store`, or `oqueue-compact`, which hold three rows of
its own hot-path table.

The second round found two more, both introduced by the fixes: the front-matter
parser **failed open** on a quoting style it did not handle — yielding a shorter
standards list with no diagnostic, which is worse than the loud failure it
already had for a missing key — and a comment justifying the rewrite made a
claim about `build-index.sh` that is **not true**. ⚠️ That leaves a real wart:
`which-standards.sh` accepts a block sequence and `build-index.sh` does not, so
a `tags:` written that way fails the index gate with a message about a missing
family tag. Reconciling the two parsers belongs to whichever task next touches
`build-index.sh`; it is recorded here rather than fixed, because doing it in
this commit would have widened it.

**M-1.32** names the rule most likely to be broken without noticing: ⚠️ **never
mix a refactor with a behaviour change**. It is the most common way an atomic
commit stops being one, it makes the diff unreviewable, and a bisect landing on
that commit cannot say which half broke. No script catches it, so it is review's
job.

It also records what happened earlier in this milestone: a pushed commit
carrying wrong counts in its message was left standing and corrected by a
follow-up rather than amended, because `main` is public and rewriting it breaks
every clone.

**M-1.6** is the first gate, and writing its negative cases immediately found
two defects in it. The subject-quality rule originally required ten characters,
and the first thing it rejected was `M-1.2: mission` — an accurate one-word
subject already in history. ⚠️ **The rule was wrong, not the commit**: length is
a proxy for meaning and a poor one, since it accepts `fix the thing` and rejects
`mission`. It is now a denylist of words that carry no information. The second
defect was a `warn` line whose backticks inside double quotes made it *execute*
`git log --oneline` rather than print it.

Both are the argument for M-1.15 in miniature: **a gate nobody has watched fail
is a gate nobody has tested.**

**M-1.15** makes that permanent. `tests/gates/negative.sh` builds one
deliberately broken artifact per existing gate — a commit subject with no
task ID, a test removed with no `Removes-test:` trailer, a threshold read
from an environment variable, a leaf crate depending on a sibling leaf, a
library crate naming `TcpStream` directly, an `oqueue-core` trait whose
signature changed with no ADR, `unsafe` outside the three allowed crates, a
staged change with no recorded review verdict, a commit no milestone-review
artifact covers, and a generated index region gone stale — and asserts each
gate's own exit code is non-zero against it. Ten cases, one per gate that
exists as of this task; `check-budget.sh`, `check-file-size.sh`, and
`check-readmes.sh` (M-1.12, M-1.27) do not exist yet and are not covered,
consistent with M-1.16's own completion condition only being able to check
what exists at the time it runs.

Every case runs in its own disposable git repository under `mktemp -d`,
never the real tree — `lib.sh` resolves `REPO_ROOT` from its own file's
location on disk, not the caller's working directory, so a gate only
resolves against the scratch repo if both `lib.sh` and the gate itself are
physically copied into it. Each case copies the real, unmodified gate
script rather than re-implementing its logic, so this suite tests the
actual file in `scripts/`, not a description of it.

⚠️ Writing this suite repeated, in miniature, the exact bug class it exists
to catch: `run_case()`'s first draft called the case function bare under
this file's own `set -e` (inherited from `lib.sh`). A case function's whole
job is to end in a command that fails — the gate under test, on a broken
artifact — so the first case run aborted the entire suite before it could
report anything, with zero output and exit 1. Fixed with `|| rc=$?`, the
same fix this session applied repeatedly elsewhere (`check-core-contract.sh`,
`check-unsafe.sh`) for the same reason: a script that must observe a
command's failure, not propagate it, cannot call that command bare under
`set -e`. The second defect was `case_core_contract`'s scratch repo lacking
a root `Cargo.toml` — `has_rust()` in `lib.sh` requires one to exist before
`check-core-contract.sh` examines anything, so the case silently `skip`ped
rather than exercising the gate at all; fixed by giving the scratch repo a
one-crate workspace manifest.

After both fixes, all ten cases passed. That alone does not distinguish a
suite that correctly detects failure from one that would report `ok`
regardless of what happened — so, following this file's own repeated
argument that a check must be watched fail before it is trusted, one case
(`case_commit_msg`) was deliberately broken to use a *valid* subject,
confirming the suite's own `run_case` reported `FAIL ... reported ok on a
broken artifact` and exited non-zero, before restoring it and reconfirming
the clean pass.

This suite is not wired into `.pre-commit-config.yaml` or CI: it is slower
than any single gate (ten scratch git repositories per run) and its purpose
is to be invoked by M-1.16's own completion condition, not to run on every
commit — adding it to the fast per-commit loop was not what this task asked
for.

Review found one more real defect after that, in the same mechanism: each
`case_*` was one function doing setup *and* the final gate invocation, its
single exit code standing for the whole thing. `run_case`'s own `set -e`
fix (above) covered the bare-call class of bug, but calling that whole
function on the left of `||` disables `set -e` for its *entire body*, not
just the call — a `copy_gate` typo mid-setup would not abort; execution
would fall through silently to the case's real final line, which then fails
for an unrelated reason (the gate binary was never copied) that reads as
"the gate correctly rejected the broken artifact." Reproduced concretely:
mistyping the script name passed to `copy_gate` in `case_commit_msg` left
the suite reporting `ok ... (exit 127)`, certifying a case whose intended
defect was never actually constructed. Fixed by splitting every case into a
`setup_*` (builds the scratch repo, echoes its path) and an `invoke_*`
(only the real gate call) — `run_case` calls `setup_*` plainly, so a setup
bug now hits this script's own `set -e` in full force and aborts the whole
suite loudly, and only `invoke_*`'s single command is ever the thing whose
exit code is caught and interpreted.

⚠️ That split alone was not enough: `dir="$(setup_fn)"` is a command
substitution, and bash does not propagate `errexit` into a command
substitution's subshell unless `shopt -s inherit_errexit` is set — found by
re-running the same typo reproduction against the split version and seeing
it *still* report a false `ok`, because `cp`'s failure inside `setup_fn`
printed straight to the terminal (outside `run_case`'s output capture,
which only wraps `invoke_fn`) but did not stop `setup_fn` from reaching its
final `printf` and returning success regardless. `shopt -s inherit_errexit`
closed it; re-running the typo case a third time now aborts the entire
suite immediately, with no case lines printed at all, rather than reporting
anything as `ok`. Both the setup-failure reproduction and the original
`case_commit_msg`-made-valid self-test were re-run clean after this fix,
and all ten shipped cases still pass.

Running the whole repository's real gates against this file, staged, before
committing (this session's standing practice, not something review asked
for) found a third defect: `check-drift.sh` scans every tracked `*.sh` file
for a threshold-shaped identifier and an environment read on the same line,
and `setup_drift`'s fixture writes exactly that pattern as a heredoc literal
— which means it exists, literally, inside `tests/gates/negative.sh` itself,
a tracked `.sh` file, and check-drift.sh failed on the real repo the moment
this file was staged. The same self-reference hazard check-drift.sh's own
header names for itself (M-1.7's retrospective: "found it did not [pass on
the repository it was being added to]"), now hit by a second file for the
same underlying reason — a gate that scans the whole tree will find its own
worked/test examples unless something stops it. `check-drift.sh` excludes
only itself by name; extending that exclusion list to every file that ever
needs to contain a fixture would erode the property that every other file,
including every other gate, is genuinely scanned. Fixed the fixture instead:
`setup_drift` now builds the `${OQUEUE_BUDGET_SECONDS:-120}` text across two
source lines (a `dollar='$'` fragment and the rest), so no single line in
`tests/gates/negative.sh` contains both signals check-drift.sh looks for,
while the two fragments still concatenate to the identical string at
runtime — which is what lands in the scratch repo's `budget.sh` and is what
check-drift.sh, run against *that* file inside the case, is meant to catch.
Reconfirmed after the fix: all ten cases still pass, and `check-drift.sh`
run directly against the real repo's staged tree passes clean.

**M-1.16** is `scripts/gates/m-1-complete.sh`, M-1's own completion
condition. It does **not** re-list which script enforces which
non-negotiable — that table already lives in `AGENTS.md`, and a second copy
here would be exactly the two-places-one-fact hazard `build-index.sh`'s own
header names for the standards and skills tables. Instead it parses
`AGENTS.md`'s `## Non-negotiables` section itself (Python, for the same
reason `build-index.sh` uses Python rather than bash to find where one
numbered rule's text ends and the next begins), collects every
`` `scripts/check-*.sh` `` a rule cites, and runs each one — asserting a rule
either names a script that exists and passes, or is rule 3, which must say
in its own words that no script can enforce it. Then it runs
`tests/gates/negative.sh` (every gate has been watched to fail, not only to
pass) and `check-milestone-review.sh --milestone M-1` (the outer loop, not
only the inner one).

The AGENTS.md parser reuses `build-index.sh`'s crash-safety wrapper
(`try`/`except Exception`, a distinct exit 3) from the start, rather than
being a fourth site M-1.45 would later need to backport it to — `check-unsafe.sh`
already established the pattern is worth reusing, not just tolerating twice.

Two defects, both caught before review by testing this script the same way
every other gate in this milestone was tested: a scratch repo built to be
genuinely, fully green (every non-negotiable script copied in and passing,
a minimal backlog naming an unrelated M-2 task so `check-commit-msg.sh`'s
own HEAD check has something real to pass, `tests/gates/negative.sh` and
every gate it exercises copied in), confirmed to make `m-1-complete.sh`
exit 0, and five separate scratch repos each breaking exactly one thing
(a rule losing its script reference, a named script that genuinely fails,
`tests/gates/negative.sh` itself failing to catch a broken artifact,
`AGENTS.md` missing its `## Non-negotiables` section, and `AGENTS.md` fed
non-UTF-8 bytes to exercise the crash wrapper specifically), each confirmed
to make it fail for the stated reason.

- A heredoc wrapped in `$(...)` had its closing `)"` written on the same
  source line as the opening `<<'PYEOF'`, which bash parsed as closing the
  command substitution *before* the heredoc body it was meant to enclose —
  `build-index.sh`'s equivalent line works because it is not wrapped in a
  substitution at all, and this script needed to be, to capture the parsed
  `RULE`/`PROBLEM` lines rather than only inspect an exit code. Bash's own
  "unterminated here-document" warning caught it immediately. Fixed by
  moving the closing `)" || rc=$?` to its own line, after the heredoc's
  `PYEOF` terminator.
- The check for whether a rule is legitimately unenforced looked for the
  literal substring "no script enforces this" in that rule's raw text, and
  rule 3's own wording wraps across a line break in `AGENTS.md`'s Markdown
  source ("No script enforces\n   this, and none can.") — so the one rule
  this check exists to exempt was the one it flagged, reported as
  `PROBLEM rule 3 names no scripts/check-*.sh script and is not marked as
  the permanent exception`. Fixed by collapsing whitespace before the
  search.

Run against the real repo, this script currently and correctly reports
**M-1 not yet complete**: every non-negotiable's gate passes and
`tests/gates/negative.sh` proves each can fail, but `check-milestone-review.sh`
finds three commits since the M-1.37 checkpoint (M-1.14, M-1.15, and the
checkpoint commit itself) that have not been read as a whole. That is a true
statement about the milestone's current state, not a defect in this script —
a milestone-review checkpoint is due, and running one is a separate action
from writing the condition that says so.

A second `milestone-review` checkpoint (`80574fc`) ran next, covering
exactly those three commits plus M-1.16 itself. It found four things: no
case in `tests/gates/negative.sh` for `m-1-complete.sh` (**M-1.46**); this
file's and `roadmap.md`'s staleness, below (**M-1.47**); fresh confirmation
that **M-1.38** and its sibling M-1.39 are still live, unfixed defects in
the literal scripts non-negotiable 4 names — worth restating because
`m-1-complete.sh` reports every non-negotiable green without them, which is
structurally correct and not the same as M-1 being done; and one minor,
untasked drift (`.pre-commit-config.yaml` hand-labels non-negotiable
numbers, the exact mapping `m-1-complete.sh` was written to avoid
hand-maintaining).

**M-1.47** is that checkpoint's own second finding, closed by this edit:
the `milestone-review` skill's "Then re-plan: amend the roadmap with what
was learned" step was skipped after the first checkpoint, so
`milestones/M-1.md`'s "Notes for the boundary review" still claimed zero
review coverage and `roadmap.md`'s M-1 task-count cell still read 44,
three commits after `reviews/` held a real 34-commit artifact saying
otherwise. Fixed by rewriting both to state what each checkpoint actually
found (with the resulting task IDs, so a reader does not have to cross-
reference `reviews/*.json` to know what happened), and by adding forward
guidance to `M-1.md` for whoever runs the boundary review before declaring
M-1 complete: a green `m-1-complete.sh` is not license to skip confirming
M-1.38 and M-1.39 are actually closed. The roadmap's task-count cell is
now stated as a snapshot rather than implied as current, since keeping it
live would just be the next thing to go stale.

**M-1.49** is checkpoint 3's own finding, closed by this edit: the same
"then re-plan" step went unrun a second time, on the same file. Checkpoint
2's own bullet 3 — added by `M-1.47`'s edit to warn that M-1.38 and M-1.39
were still live — was never revisited once both were actually fixed by this
checkpoint's own commits, so it kept asserting, present tense, a state that
had become false the moment `22389b0` and `8b0c4b6` landed. Fixed by marking
that bullet closed rather than deleting it (the finding it recorded was
real and worth keeping as history, the same way checkpoint 1's own findings
stay visible after their tasks close) and adding a Checkpoint 3 entry in the
established shape. The forward-guidance paragraph is rewritten to point at
whatever the backlog's actual open tasks are rather than naming two specific
IDs a future re-sync would have to remember to update — the exact staleness
this task exists to close, now designed against rather than merely fixed
once.

Opening the file for this also surfaced that its "Tasks" table — a
different section from the one M-1.49's own finding named — had drifted
just as far: `M-1.24`, `M-1.5`, `M-1.23`, `M-1.15`, and `M-1.16` were all
still listed under "Remaining" several commits after each landed. Same
class of staleness the finding is about, same file, found while already
there rather than left for a fourth checkpoint to catch — fixed in the same
edit, noted here rather than silently, since fixing something a finding
didn't name is still worth being explicit about. The "44 tasks" count in
this section's own lead sentence was replaced with a pointer to
`backlog.md`'s row count, matching the fix `M-1.47` already made for
`roadmap.md`'s task-count cell — the same specific number, stated in two
places, that had already gone stale once.

**M-1.23** is `standards/review.md`, turning doc 21 §3–6 into rules rather
than argument. It deliberately does not restate §3.1's deterministic/semantic
table: this repository's actual gate set is smaller than the table's aspirational
one (no `fmt`/clippy/mutation testing/coverage gates exist yet, since there is
no Rust workspace), and a copied table would either lie about what exists today
or need updating every time a new gate lands — the exact two-places-one-fact
hazard `build-index.sh`'s header already names for the standards and skills
tables. The rule is stated instead ("anything a script can decide must never
be delegated to an agent"), with a pointer to doc 21 §3.1 for the worked
example.

The standard covers both review loops in one document rather than two,
because they are the same mechanism — hash the artifact under review, spawn
an isolated reviewer, gate on a matching verdict, resolve every blocking
finding by fixing or arguing it — applied at two different scopes (one diff
against one task; one milestone's commits as a whole). Writing them as two
separate standards would have meant stating that same mechanism twice and
letting the two copies drift, which is what `check-reviewed.sh` and
`check-milestone-review.sh`'s own shared `baselines/review.txt` convention
already refused to do in the gates themselves.

`applies_to: ["*"]`, the same as `git.md`: review governs how every commit
is examined, not a subset of paths. `scripts/build-index.sh` regenerated
`AGENTS.md`'s standards table to add the new row.

⚠️ **Corrected by review**: this paragraph first claimed review.md was "the
first standard added since M-1.34 wired that generation" — wrong on both
counts, found by re-reading `git log` rather than trusting the claim.
`M-1.33` (`de169a6`) is what wired `build-index.sh`'s generation; `M-1.34`
(`216dad3`) wired `applies_to:` and `which-standards.sh`, a different
mechanism. And `M-1.5` (`7b244ad`), which added the five remaining code
standards, already used that same generation mechanism and lands after both
`M-1.33` and `M-1.34` in history — so the "first real confirmation" claim was
also false; that confirmation already happened at `M-1.5`. What remains true:
review.md is a standard added the ordinary way, one file plus a
regeneration, not a file plus a hand-edited table.

**M-1.24** is `scripts/check-requirements-trace.sh`. The data it checks
already existed — every milestone plan M-1.41 through M-1.43 wrote already
carries a `**Serves:** FR-N, NFR-N` line, confirmed by grepping all
seventeen before writing a line of the gate — so this task is the gate, not
the citations. `roadmap.md`'s own summary table gains no new column for
this: each row already links to `milestones/M-N.md`, and duplicating the
`Serves:` line into the table would be the same two-places-one-fact hazard
`build-index.sh`'s header names, now for requirement citations instead of
generated indexes. The gate reads the plans directly.

Two failure classes, both real and both tested against contrived cases in
scratch repos before review, plus the empty-requirements-table edge case
(`requirements.md` with a header row but no id rows, which would otherwise
make every plan pass vacuously for the wrong reason): a plan with no
`**Serves:**` line at all, a `**Serves:**` line naming no FR/NFR id, and a
`**Serves:**` line citing an id `requirements.md` does not list — the last
one exists because a citation is not traceable if the thing it cites is not
real, the same reasoning `check-milestone-review.sh` already applies to a
finding's `task_id`.

Given `M-1.46` exists specifically because `m-1-complete.sh` (M-1.16) shipped
without a `tests/gates/negative.sh` case, this task adds its own case in the
same commit rather than repeating that finding a third time — the one
lesson checkpoint 2 most directly named. Wired into `.pre-commit-config.yaml`
and the local hook alongside `build-index-check`, since it is cheap (a grep
over eighteen small files) and enforces exactly the kind of product-doc
staleness `M-1.47` just spent a commit fixing by hand.

Review found a real gap in the first version: `roadmap.md` already carries
a **second**, hand-maintained "Requirement coverage" table restating every
milestone's `Serves:` line as a summary row — pre-existing, and its own
prose already forward-references M-1.24 by name as the gate that should
cover it. The first version reasoned only about `roadmap.md`'s *Sequence*
table (correctly not duplicating there) and missed that this second table
already existed and already duplicated the fact with zero gate coverage —
exactly the hazard the gate's own header claimed to avoid. Extended the
same gate rather than writing a second one: it now also fails a plan whose
ids disagree with the table's row for the same milestone, a plan with no
row in the table, and a table row naming a milestone with no plan file.

⚠️ Writing that extension found its own bug before it ever reached review:
the row parser used `cut -d'|' -fN` on every line of the section, and `cut`
without `-s` prints a line **unchanged** when it contains no delimiter at
all — so the prose paragraph right below the table (which names `FR-15`,
the deferred-requirements case) was picked up as a bogus row and then
failed as a table entry for a milestone that does not exist. Reproduced in
a scratch repo built specifically to contain that trap before trusting the
parser, fixed by requiring a line to match the exact `| X | Y |` two-column
shape before treating it as a row. A second scratch repo confirmed the
fixed parser still ignores that same prose correctly, and three more
confirmed each new failure mode (a drifted id set, a plan missing its row,
an orphaned row) independently.

Round 2 review found one more real gap, in `tests/gates/negative.sh`'s own
new case rather than in the gate: adding the round-1 extension's
`[[ ! -f "$ROADMAP_FILE" ]]` check meant `setup_requirements_trace`'s
fixture — which had no `roadmap.md` at all, since it predated that check —
now failed for an unrelated reason ("roadmap.md not found") before ever
reaching the unknown-id check the case is named and commented for. The
reviewer proved this concretely with a mutant: a copy of the gate with the
unknown-id loop deleted still failed identically against the old fixture,
so `run_case`'s exit-code-only check could not tell the two apart — a case
that passes regardless of whether the thing it claims to test exists, the
same "coverage theatre" `testing.md` names for tests that execute a line
without constraining it. Fixed by giving the fixture a `roadmap.md` whose
Requirement coverage row *agrees* with the plan (both say `FR-999`, which
does not exist in `requirements.md`), so only the unknown-id path is
exercised. Reconfirmed by repeating the reviewer's own mutant: the same
gate with the unknown-id check deleted, run against the corrected fixture,
now passes — the opposite of what a real gate should do — which is what
proves the fixture is actually constraining that check now, not merely
inert.

**M-1.27** is `check-file-size.sh` and `check-readmes.sh`, `code-structure.md`
rules 13–18, 24. Both scoped to `*.rs`/`crates/*/` only — `Cargo.toml`,
`README.md`, and `AGENTS.md` appear in that standard's `applies_to` because
*other* rules in the same document govern them, not because the 500-line
limit itself does; rule 16 sits under "## Files" grouped with the
generated-table and match-arm examples that only make sense for source.
`check-file-size.sh` also enforces rule 24 (no `util`/`common`/`helpers`/
`misc` module) in the same pass, since both are properties of the same
tracked-tree scan. Both `skip` cleanly against this repository today, the
same bootstrap pattern `check-layering.sh`/`check-sans-io.sh`/
`check-core-contract.sh`/`check-unsafe.sh` already established, since no
crate exists until M0.

`check-readmes.sh` is the more speculative of the two: nothing in this
repository is a worked example of the `## Upstream` section rule 15
requires, so this task is also the first place that section's shape is made
concrete — a bullet list, one dependency per item, the name backtick-quoted
at the *start* of the line — rather than left as prose a script could not
reliably parse. Deliberately narrow: only the leading backtick token per
bullet line counts as a stated dependency, so an incidental `` `Result` ``
or `` `lib.rs` `` mentioned later in the same bullet's prose is not
mistaken for one. Tested for exactly that trap in the positive scratch
case, alongside a bullet naming `` `tokio` `` whose explanation also uses a
backtick mid-sentence.

Both gates' own test fixtures were mutant-tested before review, following
the discipline `M-1.24`'s round 2 finding established two commits ago: for
each gate, a copy with its one real check disabled (the line-length branch
neutralized to `false`; the drift-detection sets replaced with empty ones)
was run against that gate's own `tests/gates/negative.sh` fixture, and both
mutants wrongly reported `ok` — confirming each fixture is actually
constraining the behavior it claims to, not passing for an unrelated
reason. Both gates get their own case in this same commit, and both are
wired into `.pre-commit-config.yaml` and the local hook alongside the
others, for the same reason `M-1.24` gave: adding a gate without either is
the exact mistake `M-1.46` exists to have been the last instance of.

⚠️ Writing `setup_file_size`'s fixture (`tests/gates/negative.sh`) first
used `yes 'pub fn f() {}' | head -n 501` to build a 501-line file — caught
before it ever ran: `yes`'s output pipe closes the moment `head` reads its
501st line, `yes` receives `SIGPIPE`, and this file's own `pipefail`
(inherited from `lib.sh`) treats that as the pipeline's failure, which
would have aborted the whole setup function under `set -e` before ever
committing a fixture — precisely the idiom `M-1.44` exists to document,
nearly reproduced by the file whose own gate that idiom was written from.
Fixed with `printf 'pub fn f() {}\n%.0s' {1..501}`, which repeats the
format string once per brace-expanded argument and involves no pipe at all.

`check-file-size.sh` itself had a separate, unrelated bug caught the same
way, before ever reaching review: the loop called `fail()` on a violation
but then unconditionally reached the trailing `ok` line regardless, so a
run with a real violation printed a passing summary directly under its own
`FAIL` line — the exact contradiction `check-milestone-review.sh`'s header
already warns about. Reproduced with the raw (non-`grep`-filtered) output
of the over-limit scratch case, which showed both lines side by side.
Fixed with an explicit violation counter checked before the success
message, the same shape `check-requirements-trace.sh` already uses.

**M-1.28** is `scripts/profile.sh` and `scripts/bench.sh`, `performance.md`
rules 2 and 20–22. Neither is a gate — no `.pre-commit-config.yaml` entry, no
`tests/gates/negative.sh` case, matching `performance.md`'s own "Profiling:
on demand, never gated" section, which the header quotes rather than
restates.

`profile.sh <mode> <bench>` implements the exact six-row table
`performance.md` rule 20 gives. Before writing the `instructions`/`heap`/
`massif`/`cache` branch, fetched gungraun's own documentation rather than
guessing its CLI: its Callgrind/Cachegrind/DHAT/Massif backends are
selected in the benchmark harness's own Rust configuration, not by a flag
or environment variable this script could pass — so all four modes run the
identical `cargo bench --bench <bench>`, and which Valgrind sub-tool
actually executes is decided by which harness file `<bench>` names. `alloc`
names no tool the other five don't already cover and no crate exists to
wire a custom allocator into, so it reports what it needs and exits rather
than inventing a plausible-sounding command — the same discipline
non-negotiable 3 asks of test claims, extended to tooling claims.

`bench.sh <suite>` implements rule 2's three suites as one command each:
`cargo bench --bench bench-<suite> --workspace`. The workspace-wide
invocation for a bench target only some crates will define was not assumed
— built a real two-crate toy Cargo workspace (one crate with a
`bench-micro` target, one without) and ran the actual command against it,
confirming Cargo runs it for the crate that has it and silently skips the
one that doesn't rather than erroring. `cargo` is present in this
environment, so this is genuine end-to-end verification, not a scratch-repo
skip-path check — the strongest kind either script gets.

⚠️ **What neither script's testing can reach.** `valgrind`, `perf`, and
`cargo-flamegraph` are all absent from this environment, and no crate or
real benchmark exists yet, so every `require_tool` skip path in
`profile.sh` was exercised and observed to fail cleanly (in a scratch repo
with a bare `Cargo.toml`, since the real repo has none), but the actual
`cargo bench`/`cargo flamegraph` invocations for `instructions`/`heap`/
`massif`/`cache`/`flame` were not run against real code — the same limit
`M-1.12` names for `check-budget.sh`'s constant, now for command
correctness rather than a threshold. Both scripts `exec` into the
underlying `cargo` invocation rather than wrapping it, so the replaced
process's own exit code and signal handling reach the caller directly —
appropriate for a tool nobody gates, where losing `Ctrl-C` semantics to an
extra shell layer would be a real cost with no offsetting benefit.

Review found one real gap in `instructions`/`heap`/`massif`/`cache`'s
tool-presence check, despite the header's claim of having read gungraun's
docs first: those modes checked `valgrind` and `cargo` but not
`gungraun-runner`, the separate binary gungraun's own guide says its
benchmark harness needs on `PATH` — installed independently
(`cargo install gungraun-runner`), not bundled with the `gungraun` library
dependency. A developer with `valgrind`+`cargo` but not that binary would
have hit a raw gungraun error instead of the clean, remedy-bearing skip
every other missing-tool path in this file gives — undercutting rule 20's
"no ceremony" goal for exactly the developer it exists to help. Fixed with
a third `require_tool` check. Not independently exercisable in this
environment (`valgrind` is absent, so its own check fires first, same as
before the fix), but the new line's shape matches every other
`require_tool` call in this file, already proven correct elsewhere.

A minor, cosmetic finding in the same round: the `flame` mode's
`cargo-flamegraph`-missing message double-nested its parenthetical inside
`skip`'s own message text, inconsistent with every other `require_tool`
call's format in this file. Fixed to match.

**M-1.29** is `check-hot-path-bench.sh`, `performance.md` rules 18–19. ⚠️ Its
acceptance row reads literally as a hard requirement — "every hot path named
in rule 18 has a benchmark" — and implementing that literally would have made
the gate wrong rather than strict. The eight rows in rule 18's table belong to
code that does not land at once: `RecordBatch encode / decode`, CRC-32C, and
Varint decode are M2's; Compaction throughput is M5's. A gate hard-failing on
any uncovered row from the moment any crate exists would stay red from M0's
first commit until M5 ships, failing every unrelated commit in between —
unlike every other bootstrap-skip gate in this repository, whose rule is
satisfiable the instant any crate exists at all. Confirmed by grepping every
milestone plan for the table's own wording before deciding, rather than
assuming: hot-path code is genuinely scattered across M0, M2, M7, M11, and
M13's plans, not concentrated in one place.

The gate built instead checks what can be checked honestly at every point in
history: a hard failure when a `// hot-path: <name>` marker in a tracked
`.rs` file names something rule 18's table does not list (a typo, or a
benchmark comment never updated after the table changed — this direction has
no "too early" state), and a `note`, not a failure, for how many of the eight
rows have no marker anywhere yet — visible in every run's output, which is
what rule 19's "cannot silently rot" asks for read against what rot means:
the table going *inaccurate*, not the table being *incomplete* while the code
it describes has not been written. This is a narrower reading than the
acceptance criterion's literal wording, made explicitly rather than silently,
per `milestone-review`'s "if the review found the spec wrong, that is a
decision, not a task" — flagged in the script's own header and here for the
next milestone-review checkpoint or reviewer to weigh in on; the fix, if the
literal reading was actually intended, is a one-line change (`note` to
`fail`) once every hot path's owning milestone has landed.

Parsing rule 18's table surfaced a real fixture bug before review ever saw
it: the table's rows are indented four spaces (they sit inside numbered-list
item 18's continuation, not at column 0), so an initial `^\|...` regex
matched zero rows against the real file despite passing against a
hand-written test string that happened not to be indented. Caught by running
the parsing logic directly against the real `performance.md` and counting the
result (8, not 0) before wiring it into the gate, the same "verify against
the real artifact, not an assumption of its shape" discipline `M-1.24` and
`M-1.27` already established for this repository's other table-parsing
gates. Mutant-tested per that same established discipline: with the
drift-detection `fail` branch disabled, the `tests/gates/negative.sh` fixture
wrongly passes, confirming the fixture exercises the real check rather than
an incidental side effect.

⚠️ **Round 1 review found the row was closing against a spec its own
implementation admitted it did not satisfy.** The Acceptance column and
`performance.md` rule 19 both still read the literal, unnarrowed claim
("every hot path... has a benchmark," "a gate asserting each named path has
one") while the gate that shipped only half-enforced it (drift-only hard
fail; missing coverage always just a note, for all eight rows, forever) —
exactly the failure mode `sdd.md`'s "When the spec turns out to be wrong"
section exists to catch, and exactly the shape the retrospective under
`M-1.37` already flagged as fatal in effect: a row marked `done` discharges
the finding to a place `next-task` will never look again, disclosure in a
header comment notwithstanding. Fixed by rewriting the Acceptance column and
rule 19 in the same commit, per `sdd.md`'s "amend the spec and say so."

That first rewrite said the `note`→`fail` flip for a row was "that row's
owning milestone's own task" — round 2 review found this was now a *second*
copy of the same defect, one level down: prose promising per-row control the
code did not actually have, since the shipped script only computed one
aggregate uncovered-count and had no notion of an individual row's state at
all. Confirmed by reading the loop, not just the prose. Fixed for real this
time by building the mechanism the words described rather than rewriting the
words again: a `NOT_YET_BUILT` allowlist in the script itself, the same
"array entry with a reason, in the script" shape `check-file-size.sh`
already established, checked against `performance.md`'s table for the same
kind of drift a stray marker gets checked for. Every row starts allowlisted;
a row missing both a marker and an allowlist entry is now a real, individual
hard failure, verified in a scratch repo by removing one row's entry without
adding its marker and observing exactly that row fail while the other seven
stayed clean notes — the capability the docs now claim, actually exercised,
not merely asserted a second time.

A second `tests/gates/negative.sh` case was added for this new failure mode
(a required row, uncovered), and building it caught a fixture bug before
review ever saw it: the first draft's fixture table listed only one row not
in the real `NOT_YET_BUILT`, which made all eight of the script's *real*
entries "stale" against that tiny table and failed the run for the
stale-allowlist reason instead of the required-row reason the case is named
for — the exact "a fixture that fails, but for the wrong reason" bug
`M-1.24`'s round 2 review already found once in this session, now caught by
this task's own author rather than needing a second review round to surface
it. Fixed by giving the fixture's table all eight real rows verbatim plus
one extra the allowlist does not name, then confirmed correct by reading the
actual failure output before trusting it, and mutant-tested the same way as
the first case: with the required-row `fail` call disabled, the corrected
fixture wrongly passes.

Round 3 review found a third occurrence of the same defect shape, one level
further down, and this one a real gap in the mechanism rather than only in
the prose describing it: a row that is `COVERED` (has a real marker) but
still carries a `NOT_YET_BUILT` entry was never flagged, because the final
loop `continue`s past any covered row before checking whether its allowlist
entry was actually removed. Reproduced exactly as review described: add the
benchmark, forget to delete the entry — passes silently; delete the
benchmark again (a real regression) with the stale entry still present —
still passes silently, because the leftover entry keeps protecting the row
forever. This directly falsified the guarantee both rule 19 and the
script's own header state in as many words: that losing a marker again "is
a real failure for that row alone." Fixed by checking every `NOT_YET_BUILT`
entry against `COVERED` the same way it is already checked against the
table for staleness — a row that is covered but still allowlisted is now
itself a hard failure, naming the entry to remove. A third
`tests/gates/negative.sh` case covers exactly this fixture (a marker for a
row `NOT_YET_BUILT` still lists), and the full three-step sequence review
found — add marker without removing the entry (fails), remove the entry
(passes), delete the marker again (fails again, correctly, for the required-
row reason) — was run end to end in a scratch repo before trusting the fix,
not just the single new fixture in isolation. Mutant-tested the same way as
the other two cases. `.pre-commit-config.yaml`'s hook `name:` field, which
still described the gate as "drift only" after two rounds of the
enforcement growing past that, was corrected in the same commit.

**M-1.50** is checkpoint 3's second finding: all three of `check-hot-path-bench.sh`'s
checks (unknown marker, stale `NOT_YET_BUILT` entry, uncovered required row)
each ended in their own early `finish`, so a commit with two kinds of defect
at once only ever saw the first — reproduced directly, both before and after
the fix, in a scratch fixture combining an unknown marker with a stale
allowlist entry in one tree: before, only the unknown-marker `FAIL` line
appeared; after, both appear in the same run. This contradicted `lib.sh`'s
own `fail()` contract ("the caller keeps going so one run reports every
violation rather than only the first") and was inconsistent with every
sibling gate added in the same checkpoint's commits
(`check-requirements-trace.sh`, `check-file-size.sh`, `check-readmes.sh`,
`check-portability.sh`), all of which already accumulate every problem
before one terminal report — the pattern `check-hot-path-bench.sh` should
have followed from the start and did not. Fixed by replacing the three
per-check `finish` calls with one shared `problems` counter incremented in
every failure branch across all three checks, and a single `finish` at the
very end; the only conditional left is whether the closing `ok` line
prints, gated on `problems == 0`, matching `check-readmes.sh`'s exact
"accumulate, then branch once" shape. ⚠️ No new `tests/gates/negative.sh`
case: the defect this fixed is about *how many* lines a broken run prints,
not *whether* it exits non-zero, and the suite (like every other gate's
case in it) only asserts exit code — the same "message content, not exit
code" gap `M-1.30`'s round 3 already named for this file's testing
conventions. The scratch reproduction above is the actual regression test,
run and recorded rather than encoded into a framework not built to check it.

**M-1.13** implements progressive disclosure in four layers, because the
alternative — loading seven standards and 110,000 words of research into every
session — is impossible and would be useless if it were not. ⚠️ The load-bearing
piece is a skill's `description`: it is the only thing an agent sees before
deciding to load the body, so it must say *when to use this* rather than *what
this is*.

`.claude/agents/reviewer.md` is where doc 21 §4 stops being a design and starts
being a mechanism: a subagent whose prompt explicitly denies it the author's
reasoning, with a restricted tool set so it reads rather than writes.

**M-1.26** makes the structural limits real rather than aspirational.
`clippy::too_many_lines`, `cognitive_complexity`, and `too_many_arguments` are
all in the `pedantic` group, which is already denied workspace-wide, so the
thresholds in `clippy.toml` are enforced the moment a crate exists. ⚠️ The
500-line file limit needs its own script and an allowlist, because generated
protocol tables and exhaustive `match` arms over wire types are legitimately
large — M-1.27.

**M-1.28** is deliberately **not** a gate. Profiling is slow, needs a quiet
machine, and produces output requiring judgement; gating on it would either
stall the loop or produce alerts nobody trusts. The requirement is that it be
runnable at any moment without ceremony.

**M-1.25** closed a gap in the standards plan. M-1.5's list was adapted from a
service of a different shape — one that ships a single architecture, carries no
benchmarking methodology, and holds no customer key material. Four of the
categories oqueue actually needs had no standard planned at all. The remaining
M-1.5 set is now code standards only.

**M-1.21** deliberately leaves several NFRs **UNDERIVED** rather than choosing
plausible numbers. NFR-13 (aggregate throughput) blocks NFR-31 and gates
architectural decisions; NFR-55 and NFR-56 are constants that cannot be picked
before there is a workspace to measure. ⚠️ An invented number is
indistinguishable from a measured one a month later, which is why the standard
forbids it outright.

**M-1.20** corrected a conclusion rather than extending one. The first
encryption draft assumed BYOK might apply catalog-wide and derived a design from
AWS KMS's 100,000-key ceiling. At ~10,000 BYOK topics that ceiling is not
binding, and the resolution changes from *seal every region of every object* to
*segregate BYOK data into its own objects*. Both the corpus and the decision log
mark this as a correction, because a reader arriving at the earlier reasoning
would otherwise inherit a constraint that does not apply.

**M-1.17** was executed out of ID order, immediately before the first push. Task
IDs are stable, so the table is ordered by ID rather than by execution. The
substance of every borrowed practice survives — it is now stated as this
project's standard under a `[Practice]` marker, which keeps the honest
distinction from `[Assessment]` (our own reasoning) and `[Documented]`
(externally cited).
