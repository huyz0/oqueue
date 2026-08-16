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
| **T2 integration, containerized** | Docker: MinIO; a GCS emulator, name TBD ⚠️ | every push | seconds |
| **T3 real cloud** | actual S3/GCS credentials | scheduled | minutes |

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
6. **Use `oqueue-store`'s in-memory implementation for anything that would
   otherwise touch object storage.** No scratch at all beats well-managed
   scratch.

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
    the second approximately: per-push rather than nightly, and unsharded. Read
    the rule as the target and `check-mutants.sh`'s header as what runs.
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
    tests, and a failing seed is a complete replayable reproduction.
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
