---
title: "Testing"
description: >
  Read when writing any test, choosing a tier, or when a test is slow, flaky, or passes without constraining anything.
tags: [quality, tests, determinism, mutation, fakes]
applies_to: ["*.rs", "tests/*", "*/tests/*", "*/benches/*", "*oqueue-testkit/*"]
---

# Testing

What to test, at which tier, and the rules that keep the suite fast and
trustworthy. Evidence in
[docs/researches/19](../../researches/19-workspace-engineering.md) §4, §8–9.

⚠️ **In this project no human reads the code.** The suite is not a safety net
under review; it is the primary evidence that anything works. A test that
passes without constraining behaviour is worse than no test, because it produces
confidence.

## The four tiers

| Tier | Needs | Runs | Budget |
|---|---|---|---|
| **T0 unit** | nothing — pure logic, in-memory fakes | every save, every commit | microseconds |
| **T1 integration, hermetic** | in-process fakes with injected faults and latency | every commit | milliseconds |
| **T2 integration, containerized** | Docker: MinIO; a GCS emulator, name TBD ⚠️ | every push, and at the milestone boundary ⚠️ | seconds |
| **T3 real cloud** | actual S3/GCS credentials | scheduled | minutes |

⚠️ **The T2 row's cadence was "milestone boundary" for one milestone, as a
correction rather than a target, and `M1.34` restored "every push".** It said
"every push" until `M1.21` went looking for the CI job that would do it and
found `.github/workflows/gates.yml` had none and never had one — a cadence
asserted by a standard and implemented by nothing. ⚠️ **That did not mean the
T2 tests never ran**: `M1.15` and `M1.16` each record running them against a
real MinIO container by hand. What was missing is anything that *re-runs*
them. `scripts/gates/m1-complete.sh` made that invocation defined and
repeatable, and `M1.34`'s `conformance-t2` job now runs it on every push with
a MinIO service container — so both halves of the cadence are real.
⚠️ **A T2 test wired into neither is still enforced by nothing**, which is the
same shape as the claim originally corrected: the cadence holds for the suite
those two run, not for the tier by construction.

⚠️ **`fake-gcs-server` named here was found not to work for this project's
actual GCS client, writing `M1.17`** (ADR-0014): `object_store`'s GCS client
`PUT`s the XML API's bare `<bucket>/<object>` path, which `fake-gcs-server`
routes to a handler that requires an `uploadType` query parameter or a
signed-URL scheme, returning `400 invalid uploadType` otherwise — confirmed
against its published source, not assumed. Google's own `storage-testbench`
was tried next and has its own gap (its XML `PUT` response omits the
`ETag`/generation headers `object_store` reads back). Neither is fixed as of
`M1.17`; ADR-0014 has the full trace and defers finding or fixing one.

1. **T0 and T1 must carry the correctness argument.** T2 and T3 exist to catch
   *fidelity gaps in the fakes*, which is a different failure and the one that
   actually bites.
2. **Gate tiers by capability, never by `#[cfg(target_os)]`.** Use `#[ignore]`
   plus `--run-ignored all` in the tier that has the capability. ⚠️ Not a
   feature flag — a feature-gated test does not compile unless the feature is
   on, so it rots invisibly and breaks the day someone enables it.

## Deterministic simulation: the answer is a transport seam, not a runtime swap

⚠️ **This section is `M1.22`'s finding, the spike M-1 deferred to produce**,
recorded 2026-08-21 with its reasoning rather than its conclusion alone.

The question as deferred was whether `object_store`'s I/O can run under a
deterministic simulator via a `cfg`-swapped runtime, without changing crate
structure. **Under `madsim` specifically: no. But the question turns out to be
the wrong one, and the right one has a better answer.**

**Why `madsim` does not fit.** It requires `RUSTFLAGS="--cfg madsim"` *and*
replacing each I/O crate with a shim: its README lists `madsim-tokio`,
`madsim-tonic`, `madsim-etcd-client`, `madsim-rdkafka` and
`madsim-aws-sdk-s3`, and states that "all I/O-related interfaces must be
mocked during the simulation". `object_store` reaches the network through
`reqwest` → `hyper` → `hyper-util` → `tokio::net::TcpStream` (verified by
`cargo tree`; `object_store`'s own source names `TcpListener` only in its
tests). **There is no `madsim` shim for `reqwest` or `hyper`**, so the swap
would have to be a global `[patch.crates-io]` of `tokio` beneath three
third-party crates that never agreed to it — and DNS, TLS and the wall clock
sit in that path too. ⚠️ That `madsim-aws-sdk-s3` exists at all is the tell:
the ecosystem's own answer for S3 replaces the **SDK**, not the transport
under it, because simulating a real HTTP client stack is not the shape that
works.

**What actually fits, and it needs no `cfg` at all.** `object_store` exposes
a public request/response seam above the socket:

```rust
#[async_trait]
pub trait HttpService: Debug + Send + Sync + 'static {
    async fn call(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
}
pub trait HttpConnector: Debug + Send + Sync + 'static {
    fn connect(&self, options: &ClientOptions) -> Result<HttpClient>;
}
```

`AmazonS3Builder::with_http_connector` and `GoogleCloudStorageBuilder::with_http_connector`
take one. ⚠️ **The seam is not something this project's feature choice earned**,
and saying so would be the comfortable version: `with_http_connector` carries
no `cfg` beyond the module gate, and `aws = ["aws-base", "reqwest", ...]`
implies `aws-base`, so the convenience features expose exactly the same seam.
`object_store`'s feature table does describe `aws-base`/`gcp-base` as "S3/GCS
without `reqwest` or crypto; supply your own `HttpConnector` and
`CryptoProvider`" — but this workspace enables `aws-base` **plus** `reqwest`
**plus** `ring` (ADR-0012, for reasons that have nothing to do with
simulation), so that sentence does not describe our configuration. The seam is
available to any consumer of this crate on any feature set.

A deterministic harness therefore implements `HttpService` and answers
requests from an in-process model of the backend: no runtime swap, no patched
dependencies, no simulated TCP/TLS/DNS, and `oqueue-store`'s crate structure
untouched. ⚠️ **Verified by compiling and running it, not by reading**: a
throwaway probe implementing both traits and passing the connector to
`AmazonS3Builder` compiles and passes under this workspace's exact feature
set. The probes were deleted — `M1.22` is a finding, not a code task — and the
two wrinkles they turned up are recorded here so the next person does not
rediscover them, both established by compiling the failing form first:

- `HttpService` is `#[async_trait]`, so an implementor either takes that
  dependency or writes the desugared boxed-future form. The probe wrote the
  desugared form, so **no new dependency is required**.
- `HttpResponse` is `http::Response<HttpResponseBody>`, and `object_store`
  re-exports `Extensions`/`HeaderMap`/`HeaderValue` from `http` but **not**
  `Response` or `StatusCode`. `HttpResponse::new(body)` works through the
  alias, but ⚠️ `HttpResponse::builder()` does **not** — `builder()` is an
  inherent method on `Response<()>`, so it is unreachable through an alias
  whose body type is fixed. Any model of S3 must return 404s and 412s, so
  this looks like it forces a direct `http` dependency. It does not:
  `*resp.status_mut() = 404u16.try_into().expect("a valid status")` fixes the target type from
  `status_mut()`'s signature and infers `StatusCode` without the crate ever
  being named. ⚠️ `.expect(...)`, not `?`: `HttpError` has no
  `From<InvalidStatusCode>`, so the question-mark form is `E0277`. `From<Vec<u8>>` and `From<String>` for `HttpResponseBody`
  cover the body, so `bytes` and `http-body-util` are not needed either.
- ⚠️ **The *request* body is the exception, found by `M10.2`.**
  `HttpRequestBody::as_bytes()` answers `Some` only for its `Bytes` variant,
  and a PUT from `object_store` carries a `PutPayload` — so reading what the
  store actually sent needs `http_body::Body`, and `http-body-util` is a
  dev-dependency after all. The probe above never issued a PUT, which is why
  the sentence before this one is right about responses and wrong about
  requests. Symptom if rediscovered: the body stores as empty and the round
  trip fails with no hint at the cause.

⚠️ **What this does *not* buy.** The seam is above HTTP, so a model behind it
tests this project's request *construction* and response *handling* — not the
socket, TLS, connection pooling, or `object_store`'s own retry timing. Those
remain T2's job, which is why this finding does not shrink the tier table.
`turmoil` (`tokio-rs`) is the option if a future milestone needs simulated
sockets under a real hyper stack — its `axum` and `grpc` examples show hyper
running on its `tokio::net` drop-in — but nothing in `M1` needs that, and
adopting it would mean the transitive-patching problem all over again.

## Deterministic simulation: the schedule, and what a seed actually reproduces

⚠️ **This section is `M10`'s finding**, closing the "open for everything else"
rule 26 carried since `M1.22` — the object-store seam above answers *what an
S3 answers*; this answers *what order the broker's own concurrency runs in*,
which is a different question with a different mechanism (`ADR-0028`).

**The mechanism: `--cfg tokio_unstable` plus `Builder::rng_seed`, not a runtime
swap and not `biased;`.** `tokio` draws a `select!`'s branch order from a
per-runtime RNG that no seed reaches by default — measured, 200 iterations of
a two-armed `select!` with both arms ready split 102/98 under a plain
`#[tokio::test(start_paused = true)]`. `Builder::rng_seed` reaches that RNG
directly: two runtimes built from one seed produce identical branch-pick
sequences, a third seed diverges. `biased;` on the broker's own `select!`
sites was measured too and rejected — it is 200/0, but at the cost of three
client-visible behaviour changes on the serving path (`session.rs` and
`park.rs` both put a deadline-check arm first, so the keyword alone makes each
prefer staleness or timeout over a commit that landed in the same poll).
Buying determinism by changing what a client can observe is the wrong trade in
that direction. `oqueue_testkit::run_seeded` is the one caller in this
workspace of `Builder::rng_seed`; a test reaching for reproducibility uses it,
the same way `virtual_time.rs` and every seeded broker test in this milestone
does — not a hand-built `Runtime`.

**The harness is three pieces, in three crates, by what each depends on**
(`ADR-0027`) — a reader looking for "the simulation" finds a third of it in
any one place:

- **`oqueue-testkit`** (sans-I/O, `oqueue-core` and nothing else): the seed
  and the paused runtime (`seed.rs`), a `Schedule`/`Step` generator and a
  deletion-only reducer (`schedule.rs`), and the three invariants — offsets
  gap-free, the cache never ahead of durability (NFR-21), the high watermark
  never ahead of servability (`invariants.rs`) — checked against an
  `Observation` a caller assembled, never against a system this crate can
  reach itself.
- **`oqueue-store`'s `tests/it/sim/`**: the deterministic S3 — an
  `HttpService`/`HttpConnector` answering from a `HashMap`, held to the same
  conformance suite every real backend runs, plus injectable faults (a 503
  storm, a race planted inside a conditional write's window, a per-key delete
  refusal, a paused response). It is the only crate that depends on
  `object_store`, so this is the seam's only consumer.
- **`oqueue-broker`'s `tests/it/`**: the broker-level runs — a written
  sequence of faults (`invariants.rs`) and a generated one (`generated.rs`,
  drawn from a `Schedule` and reduced by `shrink` on failure) — plus the one
  worked example reachable without a `[lib]`: `bin/oqueue`'s accept loop
  noticing a dead coordinator rather than serving past it.

⚠️ **Two things a recorded seed does not survive, and a corpus entry has to
carry both** (`ADR-0028`'s Consequences): `RngSeed::from_bytes` hashes with
`std::collections::hash_map::DefaultHasher`, whose output std documents as
unstable across releases, and `select!`'s branch RNG is equally unpinned
behind `tokio_unstable` — so a `rust-toolchain.toml` bump (routine under
`build.md` rule 1) or a `tokio` patch remaps every recorded seed to a
different schedule, with **no compile error and no failing test**. A stored
`u64` means "this schedule" only against the toolchain and `tokio` version it
was recorded under; `tests/seeds/corpus.tsv` carries both pins beside every
seed for exactly this reason, and `scripts/seed-corpus.sh` warns rather than
fails when they drift, because failing on a toolchain bump would make routine
maintenance impossible.

⚠️ **And the flag is not free on the shipped binary, which an earlier draft of
`ADR-0028` said it was.** `[build] rustflags` applies to every profile, and
`cfg(tokio_unstable)` is not API-only inside `tokio`: both schedulers build a
`TaskMeta` and dispatch poll-start/poll-stop hooks on every poll under that
cfg, and `cfg_unstable_metrics!` swaps in real `SchedulerMetrics` plus
histograms. **Nothing in this project has measured that cost on the serving
path**, so a later latency regression must not be ruled out on the strength of
a "no behaviour change" claim — `M14` is where it would be measured, against
release-profile numbers rather than a debug build's.

⚠️ **What this does not buy, the same shape as the transport seam's own
limit.** A seeded schedule decides *branch order* and, since `M10.12`, *which
steps a generated run takes* — it does not make every choice point contended.
`park.rs`'s two `select!` arms are never both ready in the workload `M10.12`
built, because one produce's commit always wakes the watch before the paused
clock has reason to advance to a parked fetch's deadline; `M10.32` is open for
the workload that would change that. A seed reproducing a run is not the same
claim as a seed exploring every interleaving that run could take.

## Isolation: fakes, not mocks

3. **Prefer a fake over a mock.** A fake is a working implementation with
   simplified behaviour; a mock verifies that specific calls happened in a
   specific order. Mocks encode the implementation into the test, so the test
   breaks when you refactor and passes when you break behaviour — exactly
   backwards.
4. **Every fake lives beside the trait it implements**, in `oqueue-core`. That
   is what makes every downstream crate testable without a testkit dependency.
5. **Unit tests perform no I/O.** No socket, no clock, no filesystem, no object
   store — those arrive through injected seams. → `check-sans-io.sh` enforces
   the production side; a test that needs I/O is a signal the logic is in the
   wrong layer.
6. **Use `oqueue-core`'s `FakeObjectStore` for anything that would otherwise
   touch object storage.** No scratch at all beats well-managed scratch.
   ⚠️ ~~`oqueue-store`'s in-memory implementation~~ — this rule named a crate
   that has no such thing, and `M1.37` resolved which way it should read
   rather than building one: since `M1.8` the fake carries a `FaultConfig`
   and since `M1.10` it passes the same conformance suite as S3, so the two
   implementations `ADR-0005` distinguished converged onto one.

## Determinism — the no-flake rules

7. ⚠️ **No `sleep` in a test. Ever.** A sleep is a race you have decided to lose
   occasionally. Use `tokio::time::pause()` and `advance()`, or explicit
   synchronization. → grep gate on `sleep` in test code
8. **No fixed ports.** Bind `:0` and read back the assignment.
9. **No shared filesystem paths.** Per-test scratch under `target/tmp`. →
   `build.md` rule 19
10. **Seeded randomness, and the seed is printed on failure.** An unreproducible
    failure is a rumour.
11. **Never assert on wall-clock duration.** Schedulers differ; the assertion is
    a coin flip.
12. **Tests must pass in any order and in parallel.** The runner's
    process-per-test isolation eliminates shared-process state — most
    importantly environment variables, which are process-global and became
    `unsafe` to set in edition 2024 for exactly this reason.
13. ⚠️ **Never silently retry.** A retried test that passes is a flaky test that
    has been hidden. Flakes are reported, tracked, and fixed — a `FLAKY` result
    is a failure with extra steps.
14. **A test that takes more than a few milliseconds at T0/T1 is an architecture
    signal.** Sans-I/O logic tests in microseconds; 500 ms means it acquired
    I/O. → the per-test time threshold, which is a layering gate wearing a
    performance gate's clothes

## Test quality — does the test constrain anything?

15. **Mutation testing is the primary quality gate.** Coverage says a line ran;
    mutation says the line mattered. ⚠️ The most characteristic failure of
    generated code is a test that executes without asserting anything that could
    fail — which is *definitionally* a surviving mutant.
16. ⚠️ **"Sharded" describes an intent, not `scripts/mutants.sh`** — `--full`
    is one unsharded pass over the workspace, and the bare invocation is
    diff-narrowed. `M0.17` implemented the first half of this rule exactly and
    the second approximately: per-push rather than nightly, and unsharded.
    ⚠️ **`M4.51` moved it to nightly** — `.github/workflows/mutants.yml`, a
    `schedule:` with a declared `timeout-minutes` — after `M4.32` measured the
    workspace pass at 2294 mutants over roughly six hours against the 23 s
    figure the per-push placement had been argued from. ⚠️ **`M4.60` sharded
    it**, eight ways, with the baseline's own staleness judged once over the
    union by `scripts/check-mutants-baseline.sh` — a shard sees an eighth of
    the mutants, so the converse loop cannot run inside one. Both halves of
    this rule are implemented. Read `check-mutants.sh`'s header for what
    runs.
    **Diff-narrowed mutation runs on every commit; the full sharded run is
    nightly.** Cost becomes proportional to the change rather than to the
    codebase. → `scripts/mutants.sh`
17. **A surviving mutant is killed or argued.** Argued survivors live in a
    baseline file with a reason, and the baseline is a list nobody grows
    quietly. → `check-mutants.sh` against the baseline
18. ⚠️ **The per-test timeout under mutation must be generous.** The runner
    reports a terminated test as a failure, and cargo-mutants reads any failure
    as a kill — so a tight cap reports a **kill for a mutant nothing detected**.
    That is silent and in the worst direction. Size it against the suite under
    the parallelism the mutation run actually uses, not idle. ⚠️ This points the
    *opposite* way from rule 14's threshold; derive the two independently.
    ⚠️ **`scripts/mutants.sh` passes no `--timeout` deliberately** — the
    comment above its `args=(...)` line records why: `cargo-mutants` derives its own cap from a baseline run under
    the same parallelism, which is what this rule asks for, and a literal would
    be a threshold nobody has measured. `M0.29`.
19. **Line coverage has a per-crate floor, against a constant no environment can
    move.** Coverage is necessary and nowhere near sufficient — rule 15 is what
    makes it mean something.
20. **Property tests over example tests** wherever an invariant can be stated.

20a. ⚠️ **A gate nobody has watched fail is a gate nobody has tested**, so every
    gate has a case in `tests/gates/negative.sh` that plants the defect it
    exists to catch and observes it fail. ⚠️ **Where a fixture's output can carry
    more than one kind of failure, the case must pin the message of the defect
    it plants**, not merely a non-zero exit — otherwise the fixture passes on an
    unrelated failure while the property it exists for is deleted outright.
    `M0.21`, `M0.24` and `M0.30` each found that regression the day a gate grew
    a property. ⚠️ **The test is the fixture's output, not the gate's branch
    count**: a gate with nine `fail` branches whose fixture trips exactly one of
    them needs no pin, which is why most cases carry none. ⚠️ **The suite's pass condition is inverted**:
    a case passes when the gate fails. A fix that makes a gate *stop* failing on
    a good artifact therefore cannot be expressed here and needs a stated
    reason instead; a fix that turns a silent abort into a reported failure
    can, and "it cannot be tested" is worth one attempt at disproof before it is
    written down. → `tests/gates/negative.sh`, run per-push by
    `.github/workflows/gates.yml` and by each milestone's completion gate.
    ⚠️ **This rule was unwritten until `M0.30`** — it lived in backlog
    acceptance rows and one script header while every gate in `scripts/` was
    built against it.

    ⚠️ **Nothing checks the rule itself for more than four gates, and `M4.69`
    is why that is written here rather than assumed.** `m0-complete.sh` asserts
    "named by pre-commit or CI *and* has a `run_case`" against its own
    `M0_GATES`, which is four entries by design — the four gates `M0` added.
    Nothing extends it, so `scripts/check-idempotence-enabled.sh`,
    `scripts/check-secrets.sh` and `scripts/check-topic-list-scope.sh` appear
    in `tests/gates/negative.sh` not at all, and a branch copied from one gate
    into another arrives without the case that guarded it — which is what
    `M4.60` did with the `unviable:` exemption and what `M4.69` found by
    deleting it and watching every fixture stay green. ⚠️ **The rule binds
    regardless**: a gate with no case is a preference whether or not a script
    says so, and the reach of the check is a separate obligation from the rule
    it checks.

21. **Every `unsafe` block has a differential property test against a naive safe
    reference, kept forever** as the oracle. → `security.md` rule 19

## Specialized suites

22. **`loom` on lock-free structures.** It permutes concurrent executions
    exhaustively under the C11 memory model, deterministically, on any
    architecture. ⚠️ ARM CI is *not* the atomics gate — weak-memory bugs are
    probabilistic and rarely reproduce under ordinary load.
23. **Miri on the crates that carry `unsafe`**, with Tree Borrows. ⚠️ If it is
    too slow, cut scope, never checks — disabling the aliasing model removes the
    thing Miri is most valuable for.
24. **Fuzz targets on everything that parses untrusted bytes**, seeded from a
    corpus. Priority: request decoder, RecordBatch parser, index search over
    corrupt blobs, response round-trip. → `security.md` rule 5
25. **TSan nightly on the concurrent produce/fetch paths.** It covers Miri's
    blind spot.
26. **Deterministic simulation for cluster-level behaviour.** Node failure,
    message reordering, partitions, and clock skew are not reachable by unit
    tests, and a failing seed is a complete replayable reproduction. ⚠️ **How
    is settled for both halves this rule needs, and this row is where "open
    for everything else" closes.** Object-store I/O: see "Deterministic
    simulation: the answer is a transport seam, not a runtime swap" above
    (`M1.22`), which found the `cfg`-swapped-runtime approach this rule was
    written assuming is not available for that half. Branch order and
    generated schedules: see "Deterministic simulation: the schedule, and
    what a seed actually reproduces" above (`M10`, `ADR-0027`, `ADR-0028`) —
    `--cfg tokio_unstable` plus `Builder::rng_seed`, which *is* a
    `cfg`-swapped runtime, just not the one `madsim` would have swapped, and
    not one that changes crate structure. ⚠️ **What that buys is branch order
    and the schedule that surrounds it — a kill, per `M10.13`'s worked
    example (`bin/oqueue`'s accept loop noticing a dead coordinator loop,
    aborted rather than merely gone quiescent), and a pause, per `M10.8`'s**
    — not the other three nouns above. ⚠️ **`M10.9`'s crash points are
    storage and journal faults against a *living* coordinator, deliberately**
    — its own file says so — so a kill is neither what it injects nor what it
    measures.
    **Message reordering and clock skew are not built and nothing in this
    repository schedules them**; a future milestone that needs either starts
    from nothing. **Partitions and the lease-loss shape of node failure are
    deferred, by name, in `roadmap.md`'s deferral table**: the
    agent-coordinator partition to `M7` (there is no network between them to
    cut until a follower's index is a cache, which is `M7`'s), and lease
    expiry as an injected fault to `M6` (`M10.8`'s pause is the shape; `M6`
    supplies the lease it needs a subject).
27. **A conformance suite for every `ObjectStore` implementation**, recording
    which backends it has been run against. ⚠️ Conditional-write behaviour stays
    marked unverified until it has run against real S3.

## Structure

28. **Test names state the behaviour**, not the function: `fetch_at_high_watermark_issues_no_gets`, not `test_fetch_2`.
29. **One behaviour per test.** Several assertions about one behaviour is fine;
    several behaviours in one test means a failure does not tell you what broke.
30. **No logic in tests** — no loops that would need their own test, no
    conditionals that mean the test does something different depending on input.
31. **A test never asserts on a private implementation detail** it reached
    through `pub(crate)`. Integration tests use the public API only, which makes
    them contract tests by construction.

## What has no gate

**Whether a test tests the right thing.** Mutation testing catches tests that
constrain nothing; nothing catches a test that carefully constrains the wrong
behaviour. That is what spec-derived acceptance criteria and review are for.

**Whether a fake is faithful.** Rule 27's conformance suite is the mechanism,
but its assertions are written by hand and can be wrong in the same direction as
the fake. ⚠️ This is the highest-risk gap in the testing story.

## See also

- Test tiers and CI matrix: [docs/researches/19](../../researches/19-workspace-engineering.md) §4, §7
- Mutation testing at scale: [docs/researches/19](../../researches/19-workspace-engineering.md) §9
- Why the suite carries so much weight here: [docs/researches/21](../../researches/21-ai-development-loop.md) §7
