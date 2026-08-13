---
title: "Rust Performance Methodology: Benchmarking, Build Config, SIMD, and Unsafe Discipline"
slug: rust-performance-methodology
status: draft
last_updated: 2026-08-13
tags: [rust, benchmarking, gungraun, iai-callgrind, valgrind, simd, crc32c, pgo, lto, unsafe, miri, fuzzing, polars, performance, build-hygiene, compile-time, disk-usage]
related: [19-workspace-engineering, 20-build-and-release-portability, 21-ai-development-loop, 05-rust-ecosystem, 15-scale-architecture-position, 17-latency-budget, 02-kafka-protocol-compatibility]
summary: >
  How to measure, build, and optimize oqueue. Instruction-count benchmarking
  for noisy CI (and the hard limits: Valgrind cannot execute AVX-512 or SVE,
  so the benchmarking methodology and the SIMD work are partly incompatible);
  build/codegen config for a multi-crate workspace where crate boundaries are
  optimization barriers by default; SIMD dispatch across x86/ARM with CRC-32C
  as the one high-value target; and an unsafe policy grounded in a source
  study of Polars.
---

# Rust Performance Methodology

*Compiled 2026-08-13 from four parallel research threads: noise-resistant benchmarking, build/codegen configuration, SIMD and cross-architecture dispatch, and disciplined unsafe. Sources are cited inline per section; each thread's own report carries fuller citation.*

Marked **[Documented]**, **[Measured]** (published benchmark with methodology), **[Vendor]**, or **[Assessment]** (this corpus's reasoning).

---

## 0. Five things that change the plan

1. **`iai-callgrind` has been renamed to `gungraun`.** The old crate stopped at 0.16.1 (Jul 2025); `gungraun` is at 0.19.4 (Jul 2026) and releasing monthly. Every tutorial you find describes the old name, old env prefix (`IAI_CALLGRIND_*` → `GUNGRAUN_*`), and old output dir (`target/iai` → `target/gungraun`).
2. **Valgrind cannot execute AVX-512 or SVE.** Not "slowly" — at all. This makes instruction-count benchmarking and SIMD work **partly incompatible**, and the failure mode is silent (§2.3).
3. **GitHub-hosted runners have no PMU.** `perf stat` hardware events do not work. Confirmed in [actions/runner-images #11789](https://github.com/actions/runner-images/issues/11789).
4. **In a multi-crate workspace, crate boundaries are optimization barriers by default.** A non-generic `pub fn` without `#[inline]` is not inlinable across crates without LTO. For a byte-level codec split across crates, this is ruinous.
5. **Bounds-check elimination is worth 1–3%.** If you're trading soundness for 2%, stop. Four of our five hot paths need no unsafe at all.

---

## 1. Instruction-count benchmarking: the core methodology

### 1.1 Why, and the validation

The premise — wall-clock benchmarks are hopeless on shared cloud CI — is correct, and the proposed fix is what mature projects actually do.

**[Documented]** rustc tracks `instructions:u` as its primary metric. The framing from the compiler team is the whole methodology in one sentence:

> "This is the least realistic metric. But it has very low variance, which makes it good for detecting small changes… **You'd be mad to use it to compare the speed of two different programs, but it is very useful for comparing two slightly different versions of the same program**, which are likely to have a similar instruction mix." ([Kobzol on the rustc benchmark suite](https://kobzol.github.io/rust/rustc/2023/08/18/rustc-benchmark-suite.html))

**[Documented]** rustls runs exactly the split proposed: Cachegrind instruction counts for regression detection, plus wall time on a **dedicated bare-metal server** with TurboBoost off, HT off, governor `performance`, ASLR off — yielding **1% resolution** ([Bencher case study](https://bencher.dev/learn/case-study/rustls/)).

**[Assessment]** Note the load-bearing qualifier in the rustc quote: *similar instruction mix*. rustc is branchy, pointer-chasing, allocation-heavy scalar code. Our hot paths — CRC32C, varint decode, compression, record-batch memcpy — have a very different mix, and are precisely where "similar mix" breaks when you swap a scalar loop for a vector one.

### 1.2 gungraun

| | |
|---|---|
| Crate | `gungraun` 0.19.4 (Jul 2026); predecessor `iai-callgrind` ended at 0.16.1 |
| Requires | Valgrind ≥ 3.20 (≥ 3.22 for Cachegrind client requests); Rust 1.85.1, edition 2024 |
| Default tool | Callgrind with cache simulation; Cachegrind/DHAT/Massif also supported |
| New | A first-class **`perf` backend** — a perf-only benchmark needs no Valgrind (docs still marked TODO) |
| CI | `gungraun/setup-gungraun@v1` handles runner + Valgrind + libc debug symbols |

Metrics: `Instructions` (the only raw counter), plus derived L1/LL/RAM hits and **Estimated Cycles**.

**The Estimated Cycles formula, verified from source:**

```rust
let cycles = l1_hits + (l3_hits * 5) + (ram_hits * 35);
```

**[Assessment]** Fixed constants encoding a mid-2000s memory hierarchy: no L2-vs-L3 distinction, no memory-level parallelism, no prefetching, no store buffering. A modern core issues 4–6 instructions/cycle and overlaps dozens of outstanding misses. **Use `Ir` as the regression gate; treat Estimated Cycles as a secondary signal that something memory-shaped changed.** The one place it earns its keep: a PR where `Ir` drops but Estimated Cycles and RAM Hits rise is the classic "traded instructions for cache misses" pattern and deserves a human look. Never quote it as a performance number — the maintainers themselves call it "a rough wall-clock approximation."

### 1.3 What Callgrind does not model

**[Documented]** From the Cachegrind manual: no kernel activity ("the effect of system calls on the cache… is ignored"), no other processes, virtual rather than physical addressing, no TLB, no speculative execution, and Valgrind's thread scheduling differs from native.

Not mentioned because it's outside the design: **no out-of-order execution, no superscalar issue, no store buffers, no hardware prefetchers, no memory-level parallelism, no SIMD throughput modelling, no memory bandwidth or NUMA, no multi-core coherence.** It is a *counting* model, not a *timing* model.

**[Assessment] For oqueue specifically, three blind spots matter:**

- **I/O is invisible.** SQLite states it plainly: time spent doing I/O isn't reflected in CPU cycle counts. For a broker whose durability path is S3 PUTs, [17](17-latency-budget.md) shows the *entire* interesting latency budget sits outside this model.
- **Threads are serialized.** Valgrind runs one kernel thread at a time. Lock contention, false sharing, and cache-line ping-pong between produce and fetch paths — the things that determine broker throughput at scale — are structurally unmeasurable.
- **Async executors.** Instruction counts of a Tokio task tell you about the task, not the scheduler's wakeup behaviour or tail latency.

### 1.4 Overhead

**[Documented]** Nulgrind floor ~4×; Callgrind with cache sim **~50×**; Cachegrind 20–100×.

**[Assessment]** Design consequence: benchmark *one* representative iteration, not a loop of 10,000. The measurement is deterministic — there's no averaging benefit. This is why gungraun is often *faster* end-to-end than criterion despite the per-instruction overhead. Keep working sets modest; never touch live object storage.

---

## 2. The Valgrind SIMD wall — and why it collides with §4

This is the sharpest finding across all four threads, and it means **the benchmarking methodology and the SIMD work are partly incompatible.**

### 2.1 What Valgrind supports

**[Documented]** ([Valgrind platforms](https://valgrind.org/info/platforms.html))

| ISA | Support |
|---|---|
| x86-64 SSE → SSE4.2, AVX, **AVX2** + BMI1/BMI2/FMA | ✅ |
| **x86-64 AVX-512** | ❌ [KDE #383010](https://bugs.kde.org/show_bug.cgi?id=383010) — `CONFIRMED`, open since **Aug 2017**, 30 duplicates |
| aarch64 **NEON/ASIMD** | ✅ |
| aarch64 **SVE / SVE2** | ❌ No support, none planned |
| RISC-V | RV64GC only — no vector extension |

Patches for AVX-512 exist but are unmerged. **No stable Valgrind release includes functional AVX-512 support.**

### 2.2 Two failure modes

**Mode A — statically compiled AVX-512/SVE: hard crash.** `unhandled instruction bytes: 0x62 ...` (the EVEX prefix) → SIGILL. Loud, at least.

**Mode B — runtime dispatch: silent wrong answer.** This is the dangerous one. **Valgrind emulates `CPUID` and reports its own feature set, not the host's.** So `is_x86_feature_detected!("avx512f")` returns **false** under Valgrind even on AVX-512 hardware, your dispatcher silently selects the AVX2 or scalar path, and gungraun reports a beautifully stable instruction count **for code your production binary will never execute.**

### 2.3 The concrete risk for us

**[Assessment]** CRC-32C is our most likely SIMD consumer, and §4 recommends `crc-fast`, which dispatches to VPCLMULQDQ/AVX-512 paths on capable hardware. Compression bindings do the same. **Under Valgrind we would measure the fallback path and never know.**

Four rules follow:

1. **Never gate SIMD kernels on instruction counts.** Benchmark those with wall time on quiet hardware that has the real ISA.
2. **Assert the dispatch path** inside any gungraun benchmark whose callee does runtime feature detection — fail loudly on mismatch.
3. **Cap the CI bench binary at AVX2** (`-C target-cpu=x86-64-v3`) so what CI measures is self-consistent and cannot SIGILL.
4. **ARM is easier** — NEON works. If we ever target SVE on Graviton, the blocker returns.

### 2.4 Instruction counts aren't automatically stable in CI either

**[Documented]** CodSpeed found identical `ubuntu-24.04` runners land on **Intel Xeon 8370C or AMD EPYC 7763**, differing in L1d (48 vs 32 KiB) and ISA flags. glibc's ifunc dispatch then selects different `malloc`/`memcpy` implementations — **changing the instruction count with zero code change** ([writeup](https://codspeed.io/blog/unrelated-benchmark-regression)).

Mitigations: pin the effective ISA (`-C target-cpu=x86-64-v3`); pin glibc via a container; or self-host one runner, which makes the whole class vanish.

---

## 3. Build and codegen configuration

### 3.1 Crate boundaries are optimization barriers

**[Documented]** From the [rustc dev guide](https://rustc-dev-guide.rust-lang.org/backend/monomorph.html):

| Function kind | Inlinable across a crate boundary? |
|---|---|
| Non-generic, no attribute | **No** — MIR isn't encoded in the rlib |
| Non-generic + `#[inline]` | Yes |
| Generic (any) | Yes — monomorphized in the caller's crate |

matklad puts it bluntly: *"Without `#[inline]`, even the most trivial of functions can't be inlined across the crate boundary."*

**[Assessment] Three consequences for the workspace layout in [15](15-scale-architecture-position.md):**

1. **`lto = "thin"` is mandatory in release.** In a monolith LTO is a marginal few percent; in a 12-crate workspace it decides whether the codec inlines at all.
2. **Keep the true hot path in one crate** — record-batch encode/decode, CRC, varint, index lookup, buffer management together. Dev and `cargo test` builds have no LTO.
3. **Annotate boundary-crossers with `#[inline]`** so non-LTO builds behave sanely.

Also: **delegate public generic functions to a private non-generic inner fn**, so only a thin adapter is monomorphized per type. Worth applying to anything generic over `AsyncRead`/`AsyncWrite`/object-store backend, or every storage backend gets its own full copy of the segment-writing logic.

### 3.2 Two traps

**`lto = false` + `codegen-units = 1` performs *no LTO at all*** — not even the thin-local LTO you get by default. Setting cgu=1 and forgetting `lto` is strictly worse than the stock profile.

**`cargo bench` inherits `release`** (cgu=16, no LTO), so out of the box you benchmark codegen you never ship.

### 3.3 Measured gains

**[Measured]** From [resvg#765](https://github.com/linebender/resvg/issues/765) (Ryzen 9 5900X, 15+ pinned runs) and [typos#827](https://github.com/crate-ci/typos/issues/827):

| Change | Gain | Cost |
|---|---|---|
| LTO alone | **~8.6%** wall-clock, −14% binary | modest |
| + PGO | **14%** (resvg) to **36%** (typos) | CI complexity; ~1.25× slower instrumented runs; 60 GB of profile data for rustc-scale |
| + BOLT | **0.8–3%** | binary ~2×; **instrumentation broken on aarch64** |
| fat LTO + cgu=1 | some over thin | **~2.3× build time** |

**[Measured]** Encouraging for PGO: resvg's profile trained on one file scored *marginally better* on a different file — profiles capture branch/layout structure rather than overfitting to input.

**[Assessment] Skip BOLT.** Its wins come from icache pressure on huge codebases like rustc/LLVM; a broker's steady-state hot path is small. The aarch64 instrumentation breakage rules out Graviton without PMU access anyway.

### 3.4 `target-cpu` — and why `native` isn't reliably fastest

**[Documented]** `-C target-cpu=native` resolves to a *named CPU model*, not "every feature this chip has." Open rustc issues show it picking `apple-m1` on M2, selecting an *older* CPU on M1, and regressing on znver4.

The microarchitecture levels: **v2** = SSE4.2 (2008/2011), **v3** = AVX2/BMI2 (2013), **v4** = AVX-512.

⚠️ **v4 SIGILLs on every Intel consumer chip since Alder Lake** (AVX-512 fused off) and on Zen 2/3 cloud instances.

**[Assessment] Recommendation: `x86-64-v2` baseline.** It's free (2008-era), and critically **it makes SSE4.2 `crc32` statically available** — removing our single most important dispatch decision. Raise to v3 only if we're willing to require Haswell+. Everything above baseline goes through runtime dispatch (§4.2).

On aarch64: leave `target-cpu` at generic and set `-C target-feature=+lse,+crc`. **[Measured, AWS]** LSE atomics improve throughput "by over 3x" on larger Graviton systems versus load/store-exclusive loops — for a broker dense in atomics (offset counters, buffer refcounts, queue heads) **this is likely a bigger win than any vector kernel**. Rust ≥1.57 already enables `outline-atomics` by default on aarch64-linux, so a generic binary runtime-dispatches to LSE; setting the feature lets LLVM emit it inline.

### 3.5 Allocator

**[Measured]** mimalloc **10,704 req/s** vs tikv-jemallocator **9,981** vs glibc default in a Rust Actix benchmark — ~7% ahead.

⚠️ **jemalloc ARM page-size trap.** jemalloc bakes page size in at build time; a binary built assuming 4 KB **aborts at startup** on 64 KB-page aarch64 kernels with `<jemalloc>: Unsupported system page size`. **Polars hit exactly this** ([#5654](https://github.com/pola-rs/polars/issues/5654)). Fix: `JEMALLOC_SYS_WITH_LG_PAGE=16`.

**[Assessment]** Default to **mimalloc**, feature-gate jemalloc for heap-profiling builds (its `prof` support is genuinely valuable for a broker whose failure mode is "memory grows under a specific consumer pattern"). Benchmark **snmalloc** once — its message-passing free design matches our allocation shape exactly (network thread allocates a buffer, flush thread frees it).

But note the ordering: **buffer pooling beats allocator choice.** Most bytes should live in `bytes::Bytes` refcounted slices or a slab of fixed-size segment buffers. The allocator is worth 5–15% on residual small-object traffic.

### 3.6 Recommended profiles

Workspace root only — member-crate `[profile]` sections are ignored.

```toml
[profile.dev]
opt-level = 0
debug-assertions = true
overflow-checks  = true          # catch offset/length wraparound early
split-debuginfo  = "unpacked"

# Highest-leverage dev setting: optimize deps once, cached forever.
# Makes debug integration tests usable when zstd/lz4/crypto are in the loop.
[profile.dev.package."*"]
opt-level = 2
debug     = false

[profile.dev.build-override]     # proc macros and build scripts
opt-level = 3

[profile.release]
opt-level     = 3
lto           = "thin"           # MANDATORY in a multi-crate workspace (§3.1)
codegen-units = 16
panic         = "unwind"         # a decoder panic must not kill the broker
debug         = "line-tables-only"   # symbolicated flamegraphs in prod
strip         = "none"
overflow-checks = true           # see RUSTSEC-2026-0007 in §5.5

[profile.dist]                   # tagged artifacts; ~2.3x build time
inherits      = "release"
lto           = "fat"
codegen-units = 1

# `bench` inherits `release` by default — pin it or you benchmark
# codegen you never ship. Must differ from `dist` in exactly one way.
[profile.bench]
inherits      = "dist"
debug         = true             # REQUIRED by gungraun for symbol attribution
strip         = false

[profile.release-checked]        # CI safety net: prod codegen + assertions
inherits         = "release"
debug-assertions = true
```

**[Assessment] The governing principle: the bench profile should differ from the shipping profile in exactly one way — `debug = true` — and no other.** Every other divergence silently optimizes a different program. `debug-assertions` accidentally left on is the most common way people invalidate a Rust benchmark.

One piece of free money: **`rust-lld` is the default linker on x86-64 Linux since Rust 1.90** — **[Measured]** 7× faster linking, 40% end-to-end reduction on incremental rebuilds. Check the toolchain before configuring anything else. And **[Measured]** `mold` was **0.7% *slower*** on release builds in Depot's study; don't cargo-cult it.

### 3.7 Build hygiene: disk growth, caches, and test scratch

#### 3.7.1 Why Rust does this

**[Assessment]** Six design choices, each trading build economy for runtime performance or reproducibility. Every fix below follows from one of them, so it is worth internalizing the causes rather than memorizing the checklist:

1. **Monomorphization** — generics compile per instantiation, per crate. Generic-heavy public APIs multiply codegen.
2. **Static linking** — no shared objects; every binary embeds the full dependency closure.
3. **Every test file is its own crate** → its own binary → full static link *and* full debuginfo. Twenty integration test files means twenty copies of everything.
4. **Debuginfo on by default in dev**, covering that entire closure.
5. **No global build cache** — `target/` is per-project, so the same `tokio` compiles once per project, per profile, per worktree.
6. **Fingerprint-keyed artifacts, never collected** — a version bump orphans the old set permanently.

**[Documented] Cargo has no `target/` garbage collection — stable or nightly, and none is planned.** `cargo clean gc` exists but is nightly-gated (verified on 1.97.1) *and* collects `$CARGO_HOME` only — registry sources and git checkouts ([cargo#12633](https://github.com/rust-lang/cargo/issues/12633)).

**[Assessment] `cargo clean` is the wrong primitive.** It is all-or-nothing, so it costs a full rebuild, so people avoid running it, so disk grows. The correct primitive is **age-based pruning** — keep hot artifacts, drop cold ones. `cargo-sweep` for `target/`, `cargo-cache` for `$CARGO_HOME`.

#### 3.7.2 Where the disk goes

**[Measured]** Proportions from a 258-crate Rust workspace; they generalize.

| Sink | Observed | Fix |
|---|---|---|
| `~/.rustup` — **`rust-docs` alone is 902 MB of 1.5 GB** | 1.5 GB **per toolchain** | `rustup component remove rust-docs` |
| `target/*/deps` | 318 MB | dep debuginfo off |
| `target/*/incremental` | 253 MB — 44% of `deps` | `CARGO_INCREMENTAL=0` in CI |
| `target/*/build` — build-script output | 158 MB | dependency choice (`ring` vs `aws-lc-sys`; any `bindgen` user) |
| `$CARGO_HOME/registry/src` — extracted sources | 259 MB vs 37 MB of tarballs | regenerable; `cargo cache --autoclean` |
| Orphaned tool dirs (`mutants-tmp`, fuzz corpora, `profraw`) | 1.7 GB | age-based sweep |

**[Assessment] 60% of a toolchain is offline documentation that everyone reads on docs.rs instead** — multiplied by every nightly and beta ever installed. Highest reclaim-to-effort ratio in the ecosystem, and almost nobody does it.

**§3.6's five profiles are five independent trees**, and `bench` carries fat LTO *and* `debug = true`. Budget **30–60 GB steady state** for a broker-sized dependency graph, **per worktree**, before tooling. A direct cost of the profile split — plan for it rather than discovering it.

#### 3.7.3 Disk fixes, by leverage

1. **`[profile.dev.package."*"] debug = false`** (already in §3.6). Dependencies are ~90% of compiled code and debuginfo is 60–70% of `target/debug`. Roughly halves it, and costs nothing — nobody steps into `tokio`'s internals.
2. **Consolidate integration tests into one binary** — `tests/it/main.rs` with `mod a; mod b;` instead of `tests/a.rs`, `tests/b.rs`, …. Simultaneously the largest disk win and a large *link-time* win, since linking is what makes test builds slow. Cause (3) above.
3. **Cut duplicate dependency versions.** `cargo tree --duplicates`; 19 duplicates in a 258-crate graph is typical and means compiling e.g. both `syn` majors. Then `cargo machete` for unused deps.
4. **Give rust-analyzer its own target dir** (`"rust-analyzer.cargo.targetDir": true`). Otherwise every save contends with `cargo build` for the target lock — a serialization most people pay without noticing. Costs a second tree, buys back the inner loop.
5. **`cargo-sweep` on a timer** — the missing GC. Prunes by fingerprint mtime: `--time <days>`, `--installed`, `--recursive`. Always `--dry-run` first.
6. **Delete `dist` and `bench` between uses.** Build-on-demand, not steady state.
7. **`sccache` for worktrees and CI only.** It has what Cargo lacks — a **bounded, self-evicting LRU cache** (`SCCACHE_CACHE_SIZE`), converting unbounded growth into a fixed ceiling. But it is **incompatible with incremental compilation**, so it wins across worktrees and branch switches and loses in the inner loop. Not a global default.

#### 3.7.4 Test scratch

**[Documented]** `CARGO_TARGET_TMPDIR` points at `target/<profile>/tmp` and **Cargo creates it** — but it is set only for integration tests and benches, not unit tests in `src/`. Cover the gap with the `relative = true` trick in `.cargo/config.toml`, which resolves against the config file's own directory:

```toml
[env]
CARGO_WORKSPACE_DIR = { value = "", relative = true }
```

```rust
pub fn scratch() -> tempfile::TempDir {
    let base = option_env!("CARGO_TARGET_TMPDIR")           // integration tests, benches
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_WORKSPACE_DIR")).join("target/tmp"));
    fs::create_dir_all(&base).unwrap();
    TempDir::new_in(base).unwrap()
}
```

Three properties follow: `cargo clean` reclaims it, nothing lands in the system temp dir, and `TempDir`'s `Drop` covers the normal path — including panics, since `Drop` runs during unwind.

⚠️ **`Drop` does not survive SIGKILL**, and `cargo-nextest` kills on timeout — which is precisely how orphans accumulate. **Reap by age at suite start** rather than attempting coordination: delete anything under `target/tmp` with an mtime older than a few hours, safely older than any live run. The same discipline catches `cargo-mutants`, which copies the entire workspace per mutant into `target/mutants-tmp` and leaves it behind on interruption (observed on one workspace: **1.7 GB, over two-thirds of the entire `target/` tree**).

⚠️ **Check whether the system temp dir is a tmpfs** (it is by default on WSL2 — 16 GB observed). tmpfs is RAM. For a broker whose tests write segment files, routing scratch there is an OOM vector, not merely a cleanup annoyance. Prefer `object_store`'s in-memory backend for unit tests: no scratch at all beats well-managed scratch.

#### 3.7.5 Compile time, by leverage

**[Assessment]** Roughly in order of typical impact:

| Lever | Impact | Note |
|---|---|---|
| **Fewer dependencies** | dominant | 600 crates will never build fast. A design decision, not a flag — see [05](05-rust-ecosystem.md). |
| **Linker** | large, incremental | `rust-lld` default on x86-64 Linux since 1.90; elsewhere `mold`/`lld` explicitly |
| **`cargo check` in the edit loop** | large | never `cargo build` to find type errors |
| **`debug = 0` / `line-tables-only`** | large | shrinks debuginfo *generation* and *linking* both |
| **One integration-test binary** | large | §3.7.3 item 2 |
| **`cargo-hakari`** | large in workspaces | see below |
| **More, smaller crates** | moderate | parallelism + tighter incremental scope |
| **Outline generics** | moderate | thin generic wrapper → non-generic inner fn (§3.1) |
| **Cranelift backend** | ~30% on debug | nightly only |
| **Parallel frontend `-Zthreads`** | up to ~30% | nightly only |

**`cargo-hakari` matters specifically for our layout.** In a multi-crate workspace, building crate A then crate B can *rebuild* shared dependencies, because feature unification differs between the two invocations. A generated "workspace-hack" crate pins the union of features so each dependency compiles once. Directly implied by §3.1 pushing us toward many crates.

This all pulls **with** §3.1, not against it: many small crates give build parallelism and tight incremental scope, and thin LTO recovers the cross-crate inlining at release time. *"Keep the hot path in one crate"* is about the hot path, not the workspace shape.

The diagnostic is always `cargo build --timings` — the critical path is usually one fat leaf crate or proc-macro serialization (`serde_derive`, `async-trait`), not total crate count.

#### 3.7.6 CI and Docker are a different regime

**[Assessment]** Ephemeral builds invert the tradeoffs: incremental compilation becomes pure overhead, and the *cache* becomes the thing that grows without bound.

- `CARGO_INCREMENTAL=0` and `CARGO_PROFILE_*_DEBUG=0` always.
- **`Swatinem/rust-cache`** on GitHub Actions — it prunes to the actual dependency graph, which is why hand-rolled `actions/cache` setups grow forever and this one doesn't.
- **`cargo-chef`** for Docker — build a dependency-only layer from a recipe, then copy source, so deps cache across source changes instead of rebuilding every image.
- **`sccache`** with an S3/Redis/GHA backend; its bounded LRU is exactly right here, and the incremental incompatibility costs nothing in CI.
- **`cargo-sweep`'s stamp workflow** (stamp before build, sweep after) keeps a persistent CI cache from growing without bound.

#### 3.7.7 The standing setup

**[Assessment]** Once, globally: remove `rust-docs`, uninstall stale toolchains, install `cargo-sweep` and `cargo-cache`, put a weekly timer on age-based pruning of both trees. Per project: dependency debuginfo off, one integration-test binary, scratch under `target/tmp`, and `--timings` when it gets slow.

---

## 4. SIMD and cross-architecture dispatch

### 4.1 The landscape, and what's ruled out

**[Documented]**

- **`std::simd` is still nightly** with no stabilization date — three unresolved API blockers (lane-count bound, mask element type, swizzle API). Ruled out for a stable-toolchain binary.
- **`wide` explicitly does not support runtime dispatch** — its own docs say `is_x86_feature_detected!` and `multiversion` "do not work with wide." Compile-time specialization only.
- **`pulp`** and **`fearless_simd`** do dispatch; the latter's unforgeable ZST capability tokens are the most interesting design, but it's immature.
- **`multiversion`** is a dispatch macro, not a SIMD abstraction — it clones a function body per target and relies on **autovectorization**. Useless for intrinsic-specific kernels like CRC.

**[Assessment] Don't adopt a portable-SIMD abstraction.** Our SIMD surface is small and mostly covered by specialist crates. Take `crc-fast` and `memchr`; hand-write the one or two remaining kernels with `core::arch` + `#[target_feature]`.

Helpfully, **Rust 1.86 stabilized `target_feature_11`** (safe fns may carry `#[target_feature]`) and **1.87 made most `std::arch` intrinsics safe to call** from code with the features enabled.

### 4.2 The dispatch pattern

**[Documented]** `is_x86_feature_detected!` issues CPUID once and caches into an atomic bitset; each subsequent call is a relaxed atomic load + bit test. Cheap, but: no CSE across checks, no hoisting out of loops, and **it blocks inlining of the fast path**.

**The key inlining fact:** a `#[target_feature]` function will *not* inline into a caller lacking the features, but **it will inline callees that lack them** — recompiling them with the features enabled. So write one generic `#[inline(always)]` kernel and instantiate it inside thin `#[target_feature]` shims. One body, four specializations.

**[Assessment] Recommended shape** — resolve once into a struct of plain `fn` pointers, stored in the broker context:

```rust
#[derive(Copy, Clone)]
pub struct SimdBackend {
    pub name:   &'static str,
    pub crc32c: fn(u32, &[u8]) -> u32,
    // ...
}

impl SimdBackend {
    pub fn get() -> Self {
        static B: std::sync::OnceLock<SimdBackend> = std::sync::OnceLock::new();
        *B.get_or_init(Self::detect)
    }
}
```

Two practicalities: `#[target_feature]` fns **cannot** be coerced to `fn` pointers, so each field points at a plain wrapper — keep the granularity coarse (a whole batch, never a field). And **log `backend.name` at startup with an `OQUEUE_SIMD` env override** — that's how you benchmark all backends on one machine, how a user reproduces a corruption bug, and (per §2.3) how you assert the dispatch path inside a gungraun benchmark.

**Anti-pattern:** calling feature detection per-record. Dispatch per RecordBatch or per fetch response.

### 4.3 CRC-32C — the one high-value target

⚠️ **`crc32fast` is the WRONG polynomial.** It implements CRC-32/IEEE; **Kafka's v2 RecordBatch mandates CRC-32C (Castagnoli)**. The crate has no Castagnoli option. Using it produces batches every Kafka client rejects, with no compile error.

**[Measured]** ([corsix](https://www.corsix.org/content/fast-crc32c-4k), 4 KiB buffers)

| Implementation | Skylake @3.2 GHz |
|---|---|
| Serial (latency-bound) | 12.64 GB/s |
| PCLMULQDQ folding | 28.12 GB/s |
| Three-way split | 33.26 GB/s |
| **Fusion** (CRC unit + PCLMULQDQ on different ports, simultaneously) | **55.65 GB/s** |

With VPCLMULQDQ: **97 GB/s** on Sapphire Rapids; **85 GB/s** on Apple M1 via `neon_eor3`.

**[Assessment] At those rates CRC stops being a bottleneck.** The design lessons are therefore not "write a faster CRC" but:

1. **Use a fusion-class implementation** (`crc-fast`) so CRC costs ~1–2% instead of ~15%. A dependency choice.
2. **Never recompute a CRC you can derive.** `crc32c_combine(crc_a, crc_b, len_b)` gives the CRC of `A ‖ B` in O(log n) — invaluable when merging, splitting, or re-batching for object-storage segments. **Worth more than any instruction tuning.**
3. **Verification is mandatory on ingest; unnecessary on fetch** — serve stored bytes with the stored CRC.

### 4.4 Varints: don't SIMD them, and preferably don't decode them

**[Assessment]** Kafka varints are interleaved with variable-length payloads, so the Stream-VByte family doesn't apply. `varint-simd` is 2.5–4× faster on single varints but **reads up to 16 bytes past the varint** — a straight buffer overrun on untrusted network input — and has no runtime dispatch. Use SWAR (`0x8080…` mask + `trailing_zeros`) with a one-byte fast path. A BMI2 `pext` path is optional but must guard against AMD pre-Zen 3, where `PEXT` is microcoded at ~18 cycles.

**The bigger point: the fastest varint decoder is the one you don't call.** `offsetDelta` is relative to the batch base offset and timestamps are deltas from `firstTimestamp`, so **assigning offsets requires rewriting only the batch header** — zero varints decoded. Record-level parsing is needed only for compaction, timestamp indexing, and optional validation, so it should be opt-in per code path. **This structural decision dwarfs any SIMD work by an order of magnitude.**

### 4.5 The decision procedure

**[Assessment]** Run in order; stop at the first "no":

1. Did a profiler put this above ~5% of broker CPU under a representative workload?
2. Is it **compute-bound**? (`perf stat`: high IPC + low miss rate ⇒ yes. Low IPC + high LLC-miss ⇒ memory-bound; fix data layout instead.)
3. Is LLVM **already vectorizing** it? Check the asm. If yes, your job is to *keep* it vectorized, not hand-write.
4. Does a **maintained crate** already do this? (`crc-fast`, `memchr`, `zstd`.) You will not beat BurntSushi at `memmem`.
5. Is the buffer ≥ ~512 bytes so dispatch amortizes?
6. **Only now** hand-write — with a scalar reference and differential tests.

Two structural cautions: manual SIMD delivers ~3.8–4.0× on cache-resident data but only ~2.0–2.1× on 1B-element workloads, because **memory bandwidth dominates**; and an AVX-512 bug **will not reproduce on any developer's Intel laptop** (§3.4), making emulator CI mandatory if we ship one.

### 4.6 Index search — the real algorithmic win

**[Measured]** Static search trees report **40× over binary search on a 1 GB dataset** (1150 ns → 27 ns/query). But the win is **layout, not vector width**: 64-byte-aligned 16-element nodes, plus batching (2.5×) and prefetch (1.5×).

**[Assessment]** Kafka index entries are fixed-width `(u32, u32)` pairs — a perfect fit. Two caveats: it only pays if you **batch lookups** (fetch pipelining, not isolated seeks), and it's a *static* structure — build it when sealing a segment, use branchless binary search on the active one.

---

## 5. Unsafe discipline

### 5.1 Polars, verified against source

**[Documented]** Study at commit `1f779c9`. The widely-shared video ["How unsafe Rust made Polars 30x faster than Pandas"](https://www.youtube.com/watch?v=l6tisoOzTuk) is a **third-party explainer**, not from the Polars authors. **Two of its five claims are materially wrong:**

| Claim | Verdict |
|---|---|
| ~1,000 `get_unchecked` | ✅ **997** (839 + 158 `_mut`) — though that's only 28% of a 3,622-occurrence unchecked API family |
| Uninit buffers via `spare_capacity_mut` → `set_len` | ⚠️ Technique real, **wrong location** — 10 uses repo-wide, **zero in the aggregation kernels** cited |
| `SyncPtr` is their most bug-prone pattern | ⚠️ **Framing out of date** (see below) |
| `TrustedLen` | ✅ Accurate — and they define their own, since std's is nightly |
| `transmute` for zero-copy FFI | ❌ **Wrong** |

On transmute: Arrow C Data Interface structs are bindgen-generated `#[repr(C)]`, **built field by field** with a `private_data` + `release` ownership protocol. Only 2 transmutes exist in the FFI module, both *lifetime laundering*.

On `SyncPtr`: the modern streaming engine migrated to **`SparseInitVec`** — a **safe** abstraction where `try_set` takes `&self`, an atomic init bitmask turns a double-write from **UB into a returned `Err`**, and `try_assume_init` refuses to yield a `Vec` unless every slot was written. That migration is the real lesson.

**[Documented]** "Consumers never write unsafe" is **false for Rust consumers** — 350 `pub unsafe fn`, including `DataFrame::take_unchecked`. True only of the Python API, where PyO3 is the containment boundary.

### 5.2 The density gradient — the structural lesson

**[Documented]** Unsafe density by crate: `polars-row` 2.9%, `polars-utils` 2.1%, `polars-arrow` 1.8%, `polars-core` 1.7% … **`polars-plan` 0.05%**, and `polars-sql`/`polars-schema`/`polars-config` **0%**.

The query planner and SQL frontend are effectively unsafe-free. **[Assessment]** That maps directly onto our layout: codec and buffer primitives get unsafe; coordinator, metadata, and storage orchestration get none. Note Polars enforces this **socially** — exactly one file in the repo has `#![forbid(unsafe_code)]`. We can close that gap for free.

### 5.3 Their bugs cluster in *safe* APIs

**[Documented]** Nine soundness issues in 2026, eight filed in a single July audit week. The pattern:

- **#28187** — unsound **safe** `From` impl allowing arbitrary memory transmutation
- **#28216** — uninitialized memory exposure in **safe** `convert_columns`
- **#28218** — missing validation memory-mapping untrusted Arrow IPC files in **safe** `IpcReader`
- **#28191 / #28217 / #28778** — use-after-free and races in the async/rayon layers

**[Assessment]** The bugs are **not** at the `get_unchecked` call sites everyone worries about. They're at the **boundary where a safe API fails to validate input before feeding an unsafe core**, and in **concurrency**. #28218 is precisely the shape of "parse a Kafka RecordBatch from a hostile socket." Four of nine are in the layer where Miri is blindest.

### 5.4 Try this before reaching for unsafe

**[Measured]** Bounds-check elimination is typically worth **1–3%** ([Shnatsel's cookbook](https://github.com/Shnatsel/bounds-check-cookbook) ships assembly and hyperfine evidence per technique). **If you're trading soundness for 2%, stop.**

In order:

1. **Iterate, don't index** — 1.28× on x86 over naive indexing.
2. **Bind the slice before the loop** (`let s = &vec[..];`) — ~15% in one benchmark.
3. **Assert lengths once, up front.** Use `assert!`, **never `debug_assert!`** — the latter compiles out in release and takes the compiler's knowledge with it, forcing every check back in. Highest-leverage line in the section.
4. **Reslice both to a common length** when you can't assert equality.
5. **`#[inline(always)]` to propagate the constraint** — `rand` got 7% from "a few `assert!`s and an `#[inline(always)]`", no unsafe.
6. **`slice::as_chunks` (stable 1.88)** returns `(&[[T; N]], &[T])` — the chunk type is a fixed-size **array**, so there's nothing left to eliminate. `array_windows` stabilized in 1.94.
7. **`bytemuck::cast_slice` / `zerocopy`** for typed reinterpretation of fixed-width records — a **safe fn**, derive-checked for padding and validity. This removes the main temptation to write `from_raw_parts` in a protocol parser.

**Verify it worked:** `cargo asm --rust <path::to::fn>` and grep for `panic_bounds_check`. **[Assessment]** Consider a CI check that greps the disassembly of the five hottest functions — a cheap, durable regression guard.

**[Assessment]** Combined, `bytes` + `bytemuck` + `as_chunks`/`assert!` + `crc-fast` covers **four of our five hot paths with no unsafe at call sites.**

### 5.5 A live hazard: RUSTSEC-2026-0007

**[Documented]** `BytesMut::reserve` had an **integer overflow** — an unchecked addition in the unique-reclaim path meant `spare_capacity_mut()` handed out an **out-of-bounds slice from entirely safe calling code**. Affects `>=1.2.1, <1.11.1`.

**[Assessment]** Four lessons: the bug was in *arithmetic*, not pointers (everyone audits the `from_raw_parts`, nobody audits the `+`); a cached `cap` desynchronized from reality; **debug builds panicked while release wrapped**, so tests couldn't see it — hence `overflow-checks = true` in release (§3.6); and pin `bytes >= 1.11.1` with `cargo deny check advisories` in CI.

Corollary: **ban `unchecked_add`/`sub`/`mul`.** Polars uses them **zero** times, release builds already have overflow checks off, and we parse attacker-controlled length prefixes.

### 5.6 Tooling, and what it costs

**[Documented]** Miri catches OOB, UAF, uninit reads, misalignment, invalid values, **data races**, and — uniquely — **aliasing violations**. It cannot catch everything: it executes one path, and **passing is not a soundness proof** (it validates the executions your tests produce, not that your safe API is sound against arbitrary safe callers — exactly Polars' bug class).

⚠️ **Polars' own Miri CI runs one crate with `-Zmiri-disable-stacked-borrows`** — i.e. the aliasing model, the thing Miri is most valuable for, is switched off. **[Assessment] Don't copy that. If it's too slow, cut *scope*, not *checks*** — use `-Zmiri-tree-borrows`, which now has diagnostic parity and is more permissive.

Their SIMD-vs-Miri workaround is directly reusable: gate intrinsics behind `#[cfg(not(miri))]` with a portable fallback. Do this for the CRC path.

**[Assessment] Realistic CI budget:**

| Tool | Frequency | Cost | Verdict |
|---|---|---|---|
| clippy unsafe lints, `deny(unsafe_op_in_unsafe_fn)` | every PR | ~0 | **mandatory** |
| `#![forbid(unsafe_code)]` | compile time | 0 | **mandatory** |
| proptest, incl. differential vs. safe oracle | every PR | seconds | **mandatory** |
| `cargo careful test` (std built with debug assertions) | every PR | ~2× | **strongly recommended** |
| Miri on `-core` crates, Tree Borrows | every PR | minutes | recommended |
| cargo-fuzz smoke, 60s/target from corpus | every PR | ~2 min | recommended |
| **TSan on concurrent produce/fetch** | nightly | 5–15× | **recommended — covers Miri's blind spot** |
| Long fuzz campaigns on the decoder | nightly | hours | recommended |

**Fuzz targets in priority order:** Kafka request decoder (attacker-controlled, length-prefixed — Polars' #28218 in our codebase), RecordBatch parser, index-entry search over corrupt blobs, response encoder round-trip.

### 5.7 The policy

**[Assessment]**

**Crate containment**, `forbid` not `deny` (it cannot be locally overridden):

```
oqueue-buf         # unsafe ALLOWED — buffer primitives, refcounted slices
oqueue-codec-core  # unsafe ALLOWED — varint, frame, RecordBatch hot loops
oqueue-checksum    # unsafe ALLOWED — only if a dependency won't do
── everything else: #![forbid(unsafe_code)] ──
```

**Three unsafe crates is the budget.** A fourth requires a recorded decision. **No unsafe in the async/concurrency layer, ever** — Miri is blindest there and it's where four of Polars' nine bugs landed. `_unchecked` variants stay `pub(crate)`.

**Every unsafe block must satisfy all six:**

1. A **benchmark** showing the safe version is materially slower, with the number in the PR.
2. **§5.4 was tried first**, and the PR says which techniques failed and why.
3. A **`// SAFETY:` comment** naming the precondition and the specific check that discharges it. One unsafe op per block.
4. A **`debug_assert!`** of that precondition immediately preceding.
5. **The obligation encoded in a type where possible** — an unsafe marker trait, a validated newtype, or a safe wrapper whose bound discharges it. *Comments rot; bounds don't.* This is the `TrustedLen` pattern, and it's the most transferable idiom in Polars: **every unsafe primitive gets a safe twin whose trait bound discharges the obligation.**
6. A **differential property test** against a naive safe reference — **kept forever** as the oracle.

Review: any diff touching a `SAFETY:` block, an `unsafe impl Send`/`Sync`, or a `pub unsafe fn` signature needs a second reviewer signing off **on the safety argument**, not just the code.

> ⚠️ **SUPERSEDED for oqueue, 2026-08-13.** This rule assumed a human second reviewer. Under the fully-AI-authored methodology (decision log in [10](10-open-questions.md)) no human reads the code, so the role is filled by a **context-isolated reviewer subagent** whose verdict is bound to the staged diff by hash and enforced by a pre-commit gate — see [21](21-ai-development-loop.md) §4–5. The obligation is unchanged; only who discharges it, and how it is proven, changes. Soundness of a `SAFETY:` argument stays in [21](21-ai-development-loop.md) §3.1's semantic column, since no script can decide it.

---

## 6. The synthesis: three tensions

**[Assessment]** The four threads don't compose cleanly, and the friction points are where the real decisions live.

### Tension 1 — instruction counts vs. SIMD

**Valgrind cannot execute AVX-512 or SVE, and silently reports the wrong CPUID.** So the CI methodology and the SIMD work are partly incompatible. Resolution: **three benchmark suites, only one of which gates merges.**

| Suite | Harness | Where | Gates? |
|---|---|---|---|
| `bench-micro` — pure CPU leaves: encode/decode, varint, protocol parse, index lookup | **gungraun**, `Ir` limits | every PR, GitHub Actions | ✅ |
| `bench-macro` — end-to-end produce/fetch, batching, multi-threaded | **criterion** | nightly, quiet machine | ❌ report only |
| `bench-simd` — anything with runtime ISA dispatch | **criterion** or **tango** | quiet machine, real ISA | ❌ report only |

Gating on a metric that structurally cannot see the bottleneck trains the team to ignore alerts.

### Tension 2 — LTO vs. iteration speed

Full LTO + cgu=1 is what makes a multi-crate workspace perform like a monolith, and it's ~2.3× build time. Resolved by the three-tier profile split in §3.6: thin LTO in `release` (every merge), fat in `dist`/`bench` (nightly and tags).

### Tension 3 — unsafe for speed vs. unsafe as the bug source

Bounds-check elimination buys 1–3%; Polars' soundness bugs cluster in safe APIs that failed to validate input. **[Assessment]** The resolution is that most of our performance comes from *architecture* — not decoding varints at all, `crc32c_combine` instead of recomputation, batched index lookups, buffer pooling — and almost none from `unsafe`. Get the architecture right and the unsafe budget stays at three crates.

### Adoption order (highest ROI first)

1. **Profiles + thin LTO** (§3.6). Free, ~8.6% measured, biggest relative win in a multi-crate workspace.
2. **`crc-fast`, not `crc32fast`** — and design for `crc32c_combine` from day one.
3. **Don't decode varints on the ingest path** (§4.4). Structural; dwarfs everything else here.
4. **`gungraun` `bench-micro` suite** with `ir=1%` limits, `-C target-cpu=x86-64-v3`, dispatch assertions.
5. **mimalloc + buffer pooling.**
6. **The unsafe policy** (§5.7) — cheap to adopt at the start, expensive to retrofit.
7. **PGO** via `cargo-pgo`, driven by a realistic produce/consume load. 14–36% measured; real CI complexity.
8. **Runtime SIMD dispatch** for CRC and hashing, with the `OQUEUE_SIMD` override.
9. *(Skip)* BOLT — 0.8–3%, broken aarch64 instrumentation.
10. *(Skip)* `build-std` — nightly pin, small surface, no published server-side win.

---

## 7. Open questions

- **Do our actual dependencies dispatch to AVX-512?** `crc-fast`, `zstd`, `lz4` dispatch tables weren't verified. This determines whether §2.2 Mode B silently applies. **Verify empirically before trusting any gungraun number on a CRC or compression path.**
- **Threshold calibration.** Start `ir=1%` hard-fail with `@all=5%` warn; tighten toward 0.5% after a few weeks of green. Below ~0.5% you catch allocator/glibc drift rather than your code. Migrate to Bencher's `delta_iqr` once ≥30 historical runs exist.
- **Self-hosted runner vs. CodSpeed.** CodSpeed is free for public repos and gives instruction-count CI plus bare-metal wall-time with history and PR annotations. Trade: vendor lock-in on performance history. A single self-hosted box eliminates the CI-heterogeneity class entirely (§2.4).
- **`tango` for `bench-simd`.** Paired/interleaved A/B on the real ISA is exactly the gap gungraun can't fill. Its "1% in 1 second" claim is the author's own and unreplicated.
- **Hugepages** — no clean quantification found; measure on our workload. `transparent_hugepage=never` may be the better *stability* choice since khugepaged compaction is itself a noise source.
- **gungraun's `perf` backend** — the code module is substantial, the docs chapter is an explicit TODO.

---

## Sources

Full citations are in the four thread reports; principal sources by section:

**Benchmarking** — [gungraun](https://github.com/gungraun/gungraun) ([migration](https://github.com/gungraun/gungraun/blob/main/docs/src/migration/iai-callgrind-to-gungraun.md), [regressions](https://github.com/gungraun/gungraun/blob/main/docs/src/regressions.md), [best practices](https://github.com/gungraun/gungraun/blob/main/docs/src/best_practices.md)) · [Valgrind platforms](https://valgrind.org/info/platforms.html) · [KDE #383010 AVX-512](https://bugs.kde.org/show_bug.cgi?id=383010) · [Cachegrind manual](https://valgrind.org/docs/manual/cg-manual.html) · [Kobzol on rustc-perf](https://kobzol.github.io/rust/rustc/2023/08/18/rustc-benchmark-suite.html) · [rustls case study](https://bencher.dev/learn/case-study/rustls/) · [CodSpeed on runner heterogeneity](https://codspeed.io/blog/unrelated-benchmark-regression) · [actions/runner-images #11789](https://github.com/actions/runner-images/issues/11789) · [GCP PMU](https://docs.cloud.google.com/compute/docs/pmu-overview) · [pythonspeed on CI benchmarking](https://pythonspeed.com/articles/consistent-benchmarking-in-ci/) · [Mytkowicz et al. ASPLOS'09](https://users.cs.northwestern.edu/~robby/courses/322-2013-spring/mytkowicz-wrong-data.pdf) · [STABILIZER ASPLOS'13](https://people.cs.umass.edu/~emery/pubs/stabilizer-asplos13.pdf) · [tango](https://github.com/bazhenov/tango)

**Build/codegen** — [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) · [rustc codegen options](https://doc.rust-lang.org/rustc/codegen-options/index.html) · [rustc dev guide: monomorphization](https://rustc-dev-guide.rust-lang.org/backend/monomorph.html) · [matklad: Inline In Rust](https://matklad.github.io/2021/07/09/inline-in-rust.html) · [resvg#765](https://github.com/linebender/resvg/issues/765) · [typos#827](https://github.com/crate-ci/typos/issues/827) · [cargo-pgo](https://github.com/Kobzol/cargo-pgo) · [rust-lld default in 1.90](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable) · [AWS Graviton Rust guide](https://github.com/aws/aws-graviton-getting-started/blob/main/rust.md) · [polars#5654 jemalloc page size](https://github.com/pola-rs/polars/issues/5654)

**SIMD** — [The state of SIMD in Rust in 2025](https://shnatsel.medium.com/the-state-of-simd-in-rust-in-2025-32c263e5f53d) · [corsix fast-crc32](https://github.com/corsix/fast-crc32) / [4k analysis](https://www.corsix.org/content/fast-crc32c-4k) · [crc-fast](https://github.com/awesomized/crc-fast-rust) · [crc32c](https://github.com/zowens/crc32c) · [Kafka message format](https://kafka.apache.org/25/implementation/message-format/) · [x86-64 microarch levels](https://en.opensuse.org/X86-64_microarchitecture_levels) · [Intel AVX10 reversal](https://www.phoronix.com/news/Intel-AVX10-Drops-256-Bit) · [Rust scalable vectors goal 2026](https://rust-lang.github.io/goals/2026/scalable-vectors.html) · [static search trees](https://curiouscoding.nl/posts/static-search-tree/) · [Distributing Rust SIMD binaries](https://curiouscoding.nl/posts/distributing-rust-simd-binaries/)

**Unsafe** — [pola-rs/polars](https://github.com/pola-rs/polars) @ `1f779c9` (source study) · [SparseInitVec](https://github.com/pola-rs/polars/blob/main/crates/polars-utils/src/sparse_init_vec.rs) · [Polars soundness issues #28187/#28190/#28191/#28216/#28217/#28218/#28778](https://github.com/pola-rs/polars/issues/28218) · [Shnatsel: avoid bounds checks without unsafe](https://shnatsel.medium.com/how-to-avoid-bounds-checks-in-rust-without-unsafe-f65e618b4c1e) / [cookbook](https://github.com/Shnatsel/bounds-check-cookbook) · [RUSTSEC-2026-0007](https://rustsec.org/advisories/RUSTSEC-2026-0007.html) · [Miri](https://github.com/rust-lang/miri/) · [Ralf Jung on cargo-careful](https://www.ralfj.de/blog/2022/09/26/cargo-careful.html) · [Rustonomicon: exception safety](https://doc.rust-lang.org/nomicon/exception-safety.html) · [bytemuck](https://github.com/Lokathor/bytemuck) · [kafka-protocol-rs](https://github.com/tychedelia/kafka-protocol-rs)
