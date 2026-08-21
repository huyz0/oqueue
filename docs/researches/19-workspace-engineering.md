---
title: "Workspace Engineering: Crate Split, Contracts, Parallel Build, and Reliable Verification"
slug: workspace-engineering
status: draft
last_updated: 2026-08-13
tags: [workspace, crates, layering, contracts, sans-io, testing, determinism, flaky-tests, mutation-testing, cargo-mutants, nextest, portability, ci, build-parallelism, sccache, hakari]
related: [18-rust-performance-methodology, 20-build-and-release-portability, 21-ai-development-loop, 05-rust-ecosystem, 15-scale-architecture-position]
summary: >
  How to run a multi-crate Rust workspace: splitting crates so the build
  parallelizes, enforcing unidirectional dependencies, defining contracts
  between crates, testing each crate alone, keeping local (Linux/Mac) and CI
  verification honest, eliminating flaky tests, and making mutation testing
  cost O(change) instead of O(codebase).
---

# Workspace Engineering

*Compiled 2026-08-13. This is the engineering standard for the workspace, not a survey. Claims marked **[Practice]** are established practice in production Rust workspaces; **[Documented]** is externally cited; **[Assessment]** is this project's own reasoning.*

> **The thesis**, stated in `scripts/check-layering.sh` and worth adopting verbatim:
>
> **"A rule with no gate is a preference."**
>
> Every section below ends in a gate, because a convention that only lives in a
> document is a convention that has already been violated somewhere you haven't
> looked.

---

## 0. What this covers

| Question | Where |
|---|---|
| 1. Parallel build and build cache | §6 |
| 2. How to split crates | §1 |
| 3. Working on one crate without disturbing others | §5 |
| 4. Defining contracts between crates | §3 |
| 5. Keeping dependencies unidirectional | §2 |
| 6. Testing each crate independently | §4 |
| 7. Portable local Linux/Mac plus real CI | §7 (tests), [20](20-build-and-release-portability.md) (builds) |
| 8. Determinism and eliminating flaky tests | §8 |
| 9. Mutation testing that scales | §9 |

> **Note.** This document assumes human engineers running the workflow. oqueue is **fully AI-authored with no human reading the code** (decision log in [10](10-open-questions.md)), which changes who performs review and how it is enforced — see [21](21-ai-development-loop.md), especially §4–5 (author/reviewer separation, the review artifact gate) and §9 (cycle-time budgets). The engineering practice below is unchanged; the loop that drives it is in 21.

**[Assessment]** One caveat governs §10's additions: most of the practice below was developed for single-process services, and **oqueue is a distributed system on object storage.** Sans-I/O makes a component's state machine deterministic; it does not by itself make a five-node cluster with S3 fault modes deterministic.

---

## 1. Crate split (Q2)

### 1.1 The shape that builds fast

**[Documented]** Cargo builds a DAG of *units* and runs up to `-j N` concurrently. Two mechanics decide how well that parallelizes:

- **Pipelined compilation** (stable since 1.38): a dependent starts as soon as its dependency's `.rmeta` exists — **not** its full codegen. So the critical path is the chain of *metadata* generation, much shorter than the chain of full builds.
- **Proc-macro crates and build scripts break pipelining.** Both must be fully compiled *and* linked (or run) before any dependent can even parse. They are hard serialization points.

**[Assessment]** Therefore **DAG depth, not crate count, sets the floor on build time.** Twenty crates in a chain parallelize terribly; twenty crates in a star parallelize almost perfectly. Keep proc-macro crates few, tiny, and at the root.

### 1.2 The star topology

**[Practice]** The rule is: *every crate depends on `oqueue-core` and on nothing else in the workspace*, with named exceptions that compose.

```
                    ┌─ buf ──────┐
                    ├─ codec ────┤
   core ◄───────────┼─ checksum ─┼──── broker ──── bin/oqueue
   (no workspace    ├─ index ────┤   (composer)     (composer)
    dependencies)   ├─ store ────┤
                    ├─ coordinator
                    └─ compact ──┘
```

Depth is **2** for every library but the composer. A sixteen-package workspace built this way keeps its critical path at two links regardless of how many crates are added sideways.

**[Assessment] Split on these criteria, in order:**

1. **A contract boundary** — the crate exposes a trait others program against.
2. **Independent testability** — it can be tested without constructing the rest of the system.
3. **Build parallelism** — it removes work from the critical path.
4. **Feature gating** — a crate is Cargo's unit of optional dependency.

**Bad reasons to split:** conceptual tidiness with no dependency benefit, and anything finer than "more crates than you have cores." Note the tension with [18](18-rust-performance-methodology.md) §3.1 — crate boundaries are optimization barriers — which is resolved by thin LTO in release, not by merging crates. *"Keep the hot path in one crate"* applies to the hot path only.

⚠️ **Qualified by [ADR-0003](../internal/product/decisions/0003-leaf-crate-split-and-inlining.md), measured on 1.97.1.** Thin LTO makes a callee's IR available across the boundary, and rustc inlines functions under ~100 MIR cost units with no annotation at all — but neither covers the ~28-35 statement band, the size of a codec entry point, where `#[inline]` decides. The position holds; the resolution is the build **plus one annotation**, not merging crates.

### 1.3 Proposed oqueue layout

**[Assessment]** Same star, adapted:

| Crate | Role | unsafe |
|---|---|---|
| `oqueue-core` | Types, **every trait seam**, errors, IDs. No I/O, no async runtime. | forbid |
| `oqueue-buf` | Buffer primitives, refcounted slices | allowed |
| `oqueue-codec` | Kafka wire protocol, RecordBatch | allowed |
| `oqueue-checksum` | CRC-32C | allowed |
| `oqueue-index` | Offset→object index, search | forbid |
| `oqueue-store` | `ObjectStore` implementations (S3/GCS/memory) | forbid |
| `oqueue-coordinator` | Metadata, sequencing, recovery | forbid |
| `oqueue-compact` | Compaction planning | forbid |
| `oqueue-broker` | **Composer** — the I/O shell | forbid |
| `oqueue-testkit` | Generators and harness, **no fake** (**dev-only**) | forbid |
| `bin/oqueue` | Composition root | forbid |

Depth 2 for everything but `oqueue-broker`. The unsafe column is [18](18-rust-performance-methodology.md) §5.7's three-crate budget, and it lines up with the split for free — the crates that need `unsafe` are exactly the leaf primitives.

---

## 2. Unidirectional dependencies (Q5)

**[Documented]** Cargo already forbids *cycles* between crates — that's a hard error, so cycles are structurally impossible. What Cargo does **not** prevent is a **layering violation**: a low-level crate depending on a high-level one is perfectly acyclic and perfectly wrong.

**[Practice]** `scripts/check-layering.sh` parses each `Cargo.toml`, extracts workspace-internal runtime dependencies, and fails unless each is `-core` or the crate is a named composer. Three details matter:

- **`[dev-dependencies]` are exempt** — "a test may compose." This matters, because Cargo permits a dev-dependency cycle (crate A's dev-dep on B, B's normal dep on A) and that's what makes a shared testkit usable.
- **Test scaffolding is banned from runtime deps** — a `DEV_ONLY` list catches `testkit` appearing in `[dependencies]`, which would ship test helpers in the deployed binary.
- **Exceptions are a named list in the script**, not a pattern. An exception you have to type is an exception someone had to justify.

Write down in the script itself why it exists. This class of rule is typically believed to hold long before anything checks it, and the gap is found by audit rather than by failure.

**[Assessment]** The alternative — `cargo-deny`'s `[[bans.deny]]` with `wrappers` — can express "crate X may only be depended on by Y." It's more declarative but less readable than 60 lines of bash, and it can't express "composers are exempt" cleanly. Take the script.

**Dependency inversion is what makes the star possible.** `oqueue-core` defines `trait ObjectStore`; `oqueue-store` implements it. Core never depends on the implementation. This is the same move that lets `oqueue-coordinator` be tested against an in-memory store.

---

## 3. Contracts between crates (Q4)

### 3.1 The contract is a trait in core

**[Practice]** Every seam is a `pub trait` in `oqueue-core`, and every one carries `Send + Sync + fmt::Debug`. For oqueue the load-bearing pair is `Clock` and `ObjectStore`; a comparable service typically lands in the range of ten to fifteen such traits.

**[Assessment]** That `Debug` bound is not cosmetic — it means any component can be logged in an error path without a `where` clause fight, and it's why `missing_debug_implementations = "warn"` is in the workspace lints.

The **fakes live beside the traits**, in core. That is what makes every downstream crate testable in isolation without depending on a testkit, and it is why the testkit crate should stay nearly empty — if it grows, fakes have drifted away from the contracts they stand in for.

### 3.2 A contract change arrives whole

**[Practice]** `scripts/check-core-contract.sh` enforces that a trait change is *one atomic commit* containing the trait change, every fake, every implementation, every call site, the ADR recording why, and any dependent spec.

Two design decisions in it carry the weight:

- **It compares the `fn` signature set inside each `pub trait` block** between HEAD and the index — not the file's mtime. Editing a doc comment on a trait is not a contract change, and a rule that fires on doc comments becomes noise and gets disabled.
- **It checks only the two mechanical obligations** — implementors are present, and an ADR file is in the same commit — and explicitly declines the other four. Call sites and dependent specs are not mechanically distinguishable from ordinary edits, and pretending otherwise turns the rule into something people route around.

**[Assessment]** That second point is the more valuable lesson: **a gate that overclaims gets disabled, and then you have nothing.** Gate what's mechanical; leave the rest to review and say so out loud.

### 3.3 What to add for oqueue

**[Assessment]** Two external tools worth adding:

- **`cargo-public-api`** — snapshots the public API of each crate as text and diffs it in CI. Turns "did this change the contract?" from a judgment call into a diff. Complements `check-core-contract.sh`, which only covers traits.
- **`cargo-semver-checks`** — only if a crate is ever published to crates.io. With `publish = false` across the workspace, this is deferred.

Also: **each crate defines its own error enum with `thiserror`; only the binary uses `anyhow`.** Leaking `anyhow::Error` across a crate boundary destroys the contract, because the caller can no longer match on failure modes.

---

## 4. Independent testing per crate (Q6)

### 4.1 Sans-I/O is the enabling constraint

**[Practice]** `scripts/check-sans-io.sh` enforces that business logic touches no socket, no clock, no syscall, and — for oqueue specifically — no object store. Mechanically: **a concrete socket type named inside a library crate is a violation; a generic bound is not.** The I/O shell is generic over `AsyncRead + AsyncWrite + Unpin` and lives entirely in the one composer crate.

The characteristic finding when such a gate is first written is that the rule was already being followed — every real-time read already sat inside the one module that exists to hold it. What was missing was anything to notice if that stopped being true.

**[Assessment]** This is the single highest-leverage structural decision in the whole document, because it delivers three things at once:

1. **Isolated testing** — no crate needs a network to be tested.
2. **Determinism** — no wall clock, no scheduler, no socket means no flakes (§8).
3. **Mutation-testability** — pure state machines are exactly what mutation testing works on (§9).

The exception list is short and each entry names why it isn't business logic. *"An exception list is where a rule goes to die."*

### 4.2 The test tiers

**[Assessment]** Four tiers, gated by what they need rather than by `#[cfg(target_os)]`:

| Tier | Needs | Runs | Speed |
|---|---|---|---|
| **T0 unit** | nothing — pure logic, in-memory fakes | everywhere, every save | ms |
| **T1 integration, hermetic** | in-process fakes with injected faults/latency | everywhere, every commit | sub-second |
| **T2 integration, containerized** | Docker: MinIO, fake-gcs-server | local + CI | seconds |
| **T3 real cloud** | actual S3/GCS credentials | CI only, scheduled | minutes |

T0 and T1 must cover the correctness argument. T2 and T3 exist to catch **fidelity gaps in the fakes** — which is the failure mode that actually bites. **[Practice]** A recurring pattern in services built this way is a cluster of defects that stayed invisible because a fake answered something the real server refuses.

**[Assessment]** For oqueue the equivalent hazard is sharper, because object-storage semantics are where the design lives: conditional-write races, 503 slowdowns, eventual-consistency edges on non-AWS S3 implementations, multipart edge cases. **Budget for the fakes being wrong** — a periodic conformance suite that runs identical assertions against the fake, MinIO, and real S3, and diffs the results, is worth more than more unit tests.

### 4.3 Structure

- Unit tests in `src/` (`#[cfg(test)] mod tests`) — can reach private items.
- Integration tests in `tests/` — **public API only, so they are contract tests by construction**. If it's awkward to test from `tests/`, the public API is wrong.
- **One integration binary per crate** — `tests/it/main.rs` with `mod a; mod b;`, per [18](18-rust-performance-methodology.md) §3.7.3. Each `tests/*.rs` is otherwise a separate crate, separate link, separate copy of debuginfo.
- Doc tests are free contract examples and run under `cargo test`.

---

## 5. Parallel development on specific crates (Q3)

**[Practice]** `scripts/check-crate.sh [crate]` runs fmt and clippy scoped to a single crate, because clippy over the whole workspace on every edit is too slow to be useful as in-session feedback. CI runs the workspace-wide version regardless, so nothing goes unchecked. Per-crate `AGENTS.md` files carry context specific to that crate.

**[Assessment]** The star topology (§1) is what makes parallel work possible at all: if every crate depends only on `core`, then two people working in `oqueue-index` and `oqueue-codec` cannot conflict except through a core contract change — which §3 already forces to be atomic and ADR'd.

Three additions:

1. **`cargo-hakari`.** In a multi-crate workspace, `cargo test -p a` then `cargo test -p b` can *rebuild shared dependencies* because feature unification differs between the two invocations. A generated workspace-hack crate pins the union so each dependency compiles once. This is the single biggest quality-of-life fix for per-crate iteration, and its absence is felt exactly when people start working per-crate.
2. **`CODEOWNERS` per crate directory** — review routing follows the same boundaries as the build.
3. **Don't path-filter CI.** Running only the affected crate's tests is the classic way to get "passed CI, broke main," because the affected set is computed from paths rather than from the dependency graph. Run everything; make everything fast.

---

## 6. Build parallelism and caching (Q1)

**[Assessment]** In leverage order for a workspace specifically — general build hygiene is [18](18-rust-performance-methodology.md) §3.7.

1. **Keep the DAG wide and shallow** (§1.1). This is the only lever that changes the *floor*.
2. **Minimize proc-macro crates.** They defeat pipelining. `serde_derive` and `async-trait` are usually the critical path; `cargo build --timings` will show it.
3. **`cargo-hakari`** — feature unification, as above.
4. **`sccache`** for CI and multiple worktrees. Bounded LRU (`SCCACHE_CACHE_SIZE`), which is the one cache in the stack that self-evicts. Incompatible with incremental, so not in the inner loop.
5. **A fast linker — but check before configuring one.** On Rust ≥1.90, **`rust-lld` is already the default on x86-64 Linux**, so the common advice to wire up `mold` is now usually a regression ([18](18-rust-performance-methodology.md) §3.6: mold measured 0.7% *slower* on release builds). ⚠️ If a linker flag is set at all, it must not break the build for anyone who lacks that linker — configure it conditionally or not at all.
6. **`-Z fine-grain-locking`** (unstable, present in current cargo) locks per-unit rather than the whole target dir — relevant if concurrent per-crate builds start contending.

**⚠️ `rust-toolchain.toml` is not optional.** An MSRV in `[workspace.package] rust-version` plus a CI step that installs "the pinned toolchain" is not a pin: without the file, every contributor and every agent gets whatever `rustup` last fetched. That is both a determinism hole (§8.4) and a reproducibility hole.

---

## 7. Portability: local Linux/Mac, real verification on CI (Q7)

⚠️ **Linux-only CI is the common default, and it is not sufficient here.** Note also that a script named for "portability" often checks something else entirely — whether tooling other than one specific agent can read the repo standards is a valuable check, but it is not this one.

> **Scope.** This section covers **test** portability. **Build** portability — host toolchain, cross-compilation, the glibc floor, the artifact matrix — is [20](20-build-and-release-portability.md). The first draft of this section conflated them and covered only this half.

**[Assessment]** This matters more here than for a typical service, because [18](18-rust-performance-methodology.md) §4 puts **runtime SIMD dispatch** in the hot path: `crc-fast` selects NEON on an ARM Mac and SSE4.2/VPCLMULQDQ on x86 CI. **A CRC bug that only manifests on one backend will never be seen by a Linux-only CI**, and Apple Silicon is where most local development will happen.

### 7.1 What actually breaks between Linux and macOS

| Hazard | Consequence | Source |
|---|---|---|
| **APFS is case-insensitive by default** | A test creating `Foo` and `foo` passes on Linux, collides on Mac | [Documented] |
| **`ulimit -n` defaults to 256 on macOS** | A broker holding many segment files hits EMFILE locally and never in CI. Raise it explicitly in the test harness rather than documenting a workaround. | [Documented] |
| **`/tmp` is a symlink to `/private/tmp`** | `canonicalize()` round-trips differ; path-equality assertions break | [Documented] |
| **No `io_uring`; different async backend** | Syscall-level timing and error codes differ | [Documented] |
| **Docker is a VM, and images may be emulated** | See §7.3 — the silent one | [Documented] |
| **ARM vs x86 memory model** | aarch64 is weakly ordered where x86-64 is TSO | [Documented] |

### 7.2 ⚠️ Correction: ARM CI is not the atomics gate — `loom` is

An earlier draft of this section claimed *"ARM CI is a correctness gate, not a portability nicety"* for atomics. **That overstates it, and acting on it would leave a real gap.**

The premise is right: aarch64 is weakly ordered and will execute reorderings x86-64's TSO forbids, so a missing `Acquire`/`Release` *can* manifest there and not on x86. But **running tests on ARM is weak evidence**, because weak-memory bugs are probabilistic — they need the specific interleaving *and* the specific reordering to coincide, and ordinary test load rarely produces either.

**[Documented]** `loom` is the tool that actually addresses this. It runs a test repeatedly, **permuting the possible concurrent executions under the C11 memory model** with state-reduction to avoid combinatorial explosion — deterministically, exhaustively, and **architecture-independently**. Its own framing: some bugs *"may not surface even if you run the code millions or billions of times,"* and loom checks correctness *"under only the assumptions that the memory model gives you rather than assumptions that the current compiler or optimization or CPU might give you."*

Its limitations are real and bound where it applies: **[Documented]** slow, no fence support, an incomplete C11 model, and it requires the code under test to use `loom::sync::atomic` in place of `std` (normally via `#[cfg(loom)]`).

**[Assessment] The correct division of labour:**

| Concern | Tool | Runs on |
|---|---|---|
| Atomic orderings, lock-free algorithm correctness | **`loom`**, on small isolated data structures | any arch |
| Data races at runtime, real allocator/scheduler | **TSan** ([18](18-rust-performance-methodology.md) §5.6) | Linux CI |
| Aliasing and UB in unsafe blocks | **Miri** | any arch |
| **Codegen and SIMD dispatch divergence** | **ARM CI** | ARM runners |

ARM CI keeps its place — but for `crc-fast` picking NEON, for LSE atomics codegen, and for build breakage. Not as the concurrency gate. The design consequence is that lock-free structures should be **small and separable enough for loom to explore**, which is an argument for confining them to `oqueue-buf` and `oqueue-core`.

### 7.3 ⚠️ The silent hazard: emulated containers on Apple Silicon

**[Documented]** Testcontainers **falls back to pulling x86_64 images when no arm64 variant exists**, running them under QEMU. **[Measured]** QEMU emulation costs roughly **85% slower** execution and **2–3× more CPU**, with inflated memory and startup times.

**[Assessment]** The danger is that it *works* — so a Mac developer's T2 tier silently runs multiple times slower, timeouts calibrated on Linux start failing intermittently, and the diagnosis ("flaky test") is wrong. Two rules: **assert the container image architecture in the T2 harness and fail loudly on a mismatch**, and pin only multi-arch images (MinIO and LocalStack publish arm64; verify per-image rather than assuming).

### 7.4 ⚠️ Valgrind on macOS: partial, and the gate is per-platform anyway

An earlier claim in this conversation — *"a Mac developer cannot run the benchmark gate locally at all"* — was too strong.

**[Documented]** Upstream Valgrind has no macOS arm64 support. The community fork [`LouisBrunner/valgrind-macos`](https://github.com/LouisBrunner/valgrind-macos) does: **arm64 works on macOS 11–14 and 26 (Tahoe), with macOS 15 (Sequoia) marked experimental**; x86_64 is supported from 10.13 through 26.

**[Assessment]** But this barely matters, for a reason that is more fundamental than tooling: **instruction counts are architecture-specific.** An `Ir` count from aarch64 macOS is not comparable to one from x86-64 Linux — different ISA, different instruction mix, different libc. So even where Valgrind runs, a Mac number cannot be checked against the CI baseline.

**The rule that follows: the instruction-count gate lives on one platform — x86-64 Linux CI — and is never run locally as a gate.** Mac developers use wall-time comparison locally to find regressions, and let CI adjudicate. This is consistent with [18](18-rust-performance-methodology.md) §6's three-suite split, and it means Valgrind is not a required local tool on any platform.

### 7.5 The matrix — and it's free

**[Documented]** GitHub's **arm64 Linux hosted runners are GA and free on public repositories** (4 vCPU), and **standard GitHub-hosted runners are free and unmetered for public repos**. The much-cited macOS 10× minute multiplier applies to the *included-minutes allowance*, which public repositories do not consume.

**[Assessment]** Since oqueue is open source, **the three-way matrix costs nothing.** The cost objection to macOS CI simply doesn't apply here.

| Job | Runner | Tier |
|---|---|---|
| Tier 1 gate | `ubuntu-latest` | T0+T1, every push |
| Tier 1 gate | `ubuntu-24.04-arm` | T0+T1, every push |
| Tier 1 gate | `macos-latest` (ARM) | T0+T1, every push |
| Containerized | `ubuntu-latest` | T2, every push |
| `loom` | `ubuntu-latest` | concurrency structures, every push |
| Real cloud | `ubuntu-latest` | T3, scheduled |
| Instruction counts | `ubuntu-latest`, pinned `target-cpu` | **x86-64 Linux only** (§7.4) |

### 7.6 The gating mechanism — pick one

**[Assessment]** An earlier draft offered "a feature flag or `#[ignore]`" without choosing. Choose `#[ignore]`:

- **T0/T1** — ordinary tests. Always run, everywhere.
- **T2/T3** — `#[ignore]` with a reason, run via `cargo nextest run --run-ignored all` in the tier that has the capability.

Why not feature flags: a feature-gated test doesn't *compile* unless the feature is on, so it rots silently and breaks the moment someone enables it. An `#[ignore]`d test compiles on every build, so type errors and API drift surface immediately even where it doesn't run. Cargo features are also additive and unify across a workspace, which makes "test-only" features leak into ordinary builds.

**Never gate on `#[cfg(target_os)]`.** That is a test which silently does not exist on half your machines, and nothing reports its absence.

**T3 credentials:** use GitHub's OIDC federation to assume a scoped AWS role, not long-lived secrets. A scheduled job with permanent credentials in repo secrets is the wrong trade for a public repository.

**[Assessment]** Making local Mac and CI Linux agree is mostly about the T0/T1 tiers being genuinely hermetic — which §4.1's sans-I/O rule already delivers. If business logic touches no socket, no clock, and no filesystem, it cannot be OS-sensitive. **The portability problem shrinks to the I/O shell**, which is one crate.

---

## 8. Determinism and no flaky tests (Q8)

### 8.1 Structural: sans-I/O plus injected seams

**[Practice]** `Clock` is a `pub trait` in core, and the sans-I/O gate ensures nothing else reads the real time. That eliminates the largest single class of flakes by construction rather than by discipline.

**[Assessment] For oqueue, extend the rule from "no socket, no clock, no syscall" to include "no object store."** `ObjectStore` becomes a core trait exactly like `Clock`, and every fake can then inject latency, 503s, conditional-write races, and partial failures deterministically. This is also what makes the §4.2 fidelity-gap problem tractable.

### 8.2 Mechanical rules

**[Assessment]**

- **No `sleep` in tests, ever.** Use `tokio::time::pause()` + `advance()`, or explicit synchronization. A `sleep` is a race you've decided to lose occasionally.
- **No fixed ports.** Bind `:0`, read back the assignment.
- **No shared paths.** Per-test scratch under `target/tmp` ([18](18-rust-performance-methodology.md) §3.7.4).
- **Seeded randomness, with the seed logged** on failure.
- **`cargo-nextest`'s process-per-test isolation** is the single biggest anti-flake lever in Rust, because it eliminates shared-process state — most importantly environment variables, which are process-global, are mutated by tests, and became `unsafe` to set in edition 2024 precisely because of this.

### 8.3 Deterministic simulation testing — the oqueue-specific addition

**[Assessment]** Sans-I/O makes a *component* deterministic. It does not make a *cluster* deterministic: node failures, message reordering, partitions, and clock skew across five nodes are the bugs that matter for [13](13-coordinator-recovery.md)'s recovery paths and [15](15-scale-architecture-position.md)'s metadata sharding, and no amount of unit testing reaches them.

DST — a seeded, single-threaded deterministic executor driving the whole system with injected faults — is the technique FoundationDB, TigerBeetle, and Antithesis are built on, and it's already surveyed in [05](05-rust-ecosystem.md) §9 (`madsim`, `turmoil`). **A failing seed is a complete, replayable reproduction**, which is the property no other testing approach in this document provides.

This is the largest genuinely new testing investment the project needs, and the argument for it is that the corpus's hardest open questions — coordinator failover RTO, metastable failure, the enumeration fork — are all cluster-level and otherwise only observable in production.

### 8.4 Deterministic *builds*

**[Assessment]** Commit `Cargo.lock`; add `rust-toolchain.toml` (§6); use `--locked` in CI so a stale lockfile fails loudly instead of silently resolving; `cargo-deny` for supply chain. For byte-reproducible artifacts, `--remap-path-prefix` and `SOURCE_DATE_EPOCH`.

---

## 9. Mutation testing (Q9)

### 9.1 Do we need it?

**[Assessment] Yes, and scoped.** Coverage says a line ran; mutation testing says the line *mattered*. For oqueue the payoff concentrates in exactly the places a subtle wrong answer is catastrophic and invisible: offset arithmetic, CRC boundaries, index binary search, retention predicates, conditional-write fencing. `<` vs `<=` in an index lookup is a data-loss bug that every coverage report will call fully covered.

**[Practice]** The empirical argument for it: in services built this way, the defects that survive every other gate are consistently of one shape — a fake answered something the real dependency refuses, or a fix went in half-applied and green. Each of those is a line whose removal would change nothing any test could observe, which is exactly what a surviving mutant is.

Priority order: property tests first, DST second (§8.3), mutation testing third. It is a *test-quality* audit, not a bug finder.

### 9.2 Making it O(change), not O(codebase)

This is the direct answer to "how do we keep it from slowing down as the code grows."

**[Practice]** A full run on a workspace of this size is several thousand mutants, each costing a build plus a test run. Four mechanisms make that affordable:

| Mechanism | Effect |
|---|---|
| **Diff narrowing** — `MUTANTS_DIFF=<(git diff origin/main...)` | **Cost becomes proportional to the change, not the codebase.** A normal PR touches tens of lines; mutating those takes minutes. This is the whole answer. |
| **Sharding** — `MUTANTS_SHARD k/n`, 4 shards nightly | Wall-clock ÷ n. cargo-mutants randomizes *after* sharding so slices cost about the same. |
| **Scoping** — an explicit crate list | Only pure state machines; the criterion is stated and the exclusions are written down. |
| **A baseline file** — accepted survivors with reasons | Keys carry **no line numbers**, so editing the lines above a survivor doesn't invalidate its entry. New survivors fail the run. |

CI shape: **`mutants-diff` on every PR, full sharded run nightly.** The diff run *"is a narrowing, not a different check"* — same baseline, same verdicts. What it cannot see is a mutant your change made survivable *elsewhere*, which is why the nightly run still exists.

### 9.3 Two hazards that cost real debugging time

**⚠️ Disk exhaustion.** cargo-mutants copies the whole build tree per worker. A build tree with coverage instrumentation can reach ~30 GB, so six workers against a 16 GB tmpfs — the default `/tmp` on many systems, including WSL2 — exhausts it and the run dies partway with `No space left on device`, *after* having already paid for the builds. Fix: redirect `TMPDIR` to `target/mutants-tmp` on real disk. This is also a source of the orphaned trees in [18](18-rust-performance-methodology.md) §3.7.2.

**⚠️ The false-kill hazard — the non-obvious one.** cargo-mutants reads *any* test failure as "the mutant was caught." nextest reports a terminated test as a failure. Therefore **a per-test timeout that is too tight reports a kill for a mutant that nothing detected** — a silent false negative in the direction the entire system exists to prevent.

**[Practice]** It is found empirically and by accident: a mutant reported as caught in one full run and missed in a targeted run, on identical code. Run by hand against the whole suite ten times, the mutant survives ten times — so the *kill* was the anomaly, not the miss.

The root cause is that the timeout must be sized against the suite **under the parallelism the mutation run actually uses**, not idle. Measured on one workspace:

```
idle                      slowest test 2.85s
six concurrent suites     slowest test 6.66s   ← MUTANTS_JOBS=6
```

A 10s cap that looked like 48× headroom when the suite was small had become **1.5×**. The fix was 30s with `terminate-after = 2`. The asymmetry is the lesson: *"The cost of being generous here is bounded and small… The cost of being tight is a false kill, which is silent."*

**[Assessment]** There must also be a per-*run* backstop (`MUTANTS_TIMEOUT`) so a genuinely hung test costs one mutant rather than the run — and the two timeouts have to be reasoned about together, which is why a correct `.config/nextest.toml` here is dozens of lines of reasoning for one line of config.

---

## 10. What oqueue must add

**[Assessment]** Adopt everything above. Four genuine additions, in priority order:

1. **Deterministic simulation testing** (§8.3). The largest investment and the largest payoff, because the corpus's hardest open questions are cluster-level.
2. **`loom` on the lock-free structures** (§7.2), plus a multi-OS/multi-arch CI matrix (§7.5) — which is free on a public repo. ARM CI covers codegen and SIMD divergence; loom covers atomics.
3. **An `ObjectStore` trait seam in core** (§8.1), extending the sans-I/O rule so object storage is injected exactly like `Clock` — plus a conformance suite diffing fake vs MinIO vs real S3 (§4.2).
4. **`cargo-hakari`, `rust-toolchain.toml`, and `sccache`** (§6) — cheap, and each closes a gap that conventional setups leave open.

Plus one **removal**: the commented-out mold configuration is obsolete on Rust ≥1.90 and would likely be a regression if enabled.

---

## 11. Open questions

- **Where does DST go in the crate graph?** `madsim` works by substituting the runtime at compile time (`--cfg madsim`), which affects every crate that touches async. Whether that composes with the sans-I/O split — or makes it partly redundant — needs a spike before committing. ⚠️ **ANSWERED 2026-08-21 by `M1.22`, for object-store I/O** (`docs/internal/standards/testing.md`, "Deterministic simulation"): not in the crate graph. `madsim` needs a shim per I/O crate and none exists for the `reqwest`/`hyper` stack `object_store` uses, so the compile-time substitution described here is unavailable; `object_store`'s own `HttpService` seam is what works, and it composes with the sans-I/O split rather than competing with it. ⚠️ The broker's own sockets are a separate stack and still open; `turmoil-net` is a `tokio::net` drop-in, which *is* crate-graph-relevant, and `M10.md` still lists that as needing an ADR.
- **Fake fidelity for object storage.** How do we *know* the in-memory `ObjectStore` matches S3 on conditional-write races? A conformance suite is proposed in §4.2 but not designed, and this is the highest-risk fake in the system.
- **Mutation-testing budget as the codebase grows.** Diff narrowing bounds per-PR cost, but the nightly full run grows linearly. At what mutant count does 4 shards stop being enough, and is the answer more shards or a tighter crate list?
- **Does the star topology survive oqueue's size?** A comparable service holds around a dozen traits in `core` comfortably. If ours reaches ~40, `core` becomes a rebuild bottleneck for the whole workspace — every change to it invalidates everything. Watch for it; the fix is splitting `core` into `core-types` (rarely changes) and `core-traits`.
- **`cargo-public-api` adoption** (§3.3) — worth it, or does `check-core-contract.sh` plus review already cover the contract surface?

---

## Sources

Practices marked **[Practice]** are drawn from production Rust workspaces of comparable size and are stated here as this project's standard rather than as a report on any particular codebase.

**External** — [Cargo workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html) · [pipelined compilation (rust#60988)](https://github.com/rust-lang/rust/issues/60988) · [cargo-hakari](https://docs.rs/cargo-hakari/) · [cargo-nextest](https://nexte.st/) · [cargo-mutants](https://mutants.rs/) · [cargo-public-api](https://github.com/enselic/cargo-public-api) · [cargo-deny](https://embarkstudios.github.io/cargo-deny/) · [sccache](https://github.com/mozilla/sccache) · [madsim](https://github.com/madsim-rs/madsim) · [turmoil](https://github.com/tokio-rs/turmoil) · [loom](https://github.com/tokio-rs/loom) · [valgrind-macos fork](https://github.com/LouisBrunner/valgrind-macos) · [arm64 runners GA for public repos](https://github.blog/changelog/2025-08-07-arm64-hosted-runners-for-public-repositories-are-now-generally-available/) · [sans-I/O](https://sans-io.readthedocs.io/) · [TigerBeetle on DST](https://tigerbeetle.com/blog/2023-03-28-random-fuzzy-thoughts/)
