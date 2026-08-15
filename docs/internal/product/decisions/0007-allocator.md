# 0007. The global allocator

Status: accepted
Date: 2026-08-16
Requirements: NFR-40, NFR-42

## Context

A broker's steady state is small-object churn: a buffer allocated on the network
thread, filled, handed to a flush thread, freed there. glibc's allocator is not
tuned for that pattern, and doc 18 §3.5 measures **mimalloc at 10,704 req/s
against tikv-jemallocator's 9,981** in a Rust Actix benchmark — about 7%.

⚠️ **Ordering first, because it changes how much this matters.** Doc 18 §3.5 is
explicit that **buffer pooling beats allocator choice**: most bytes should live
in refcounted slices or a slab of fixed-size segment buffers, and the allocator
is worth 5-15% on the *residual* small-object traffic. This decision is
therefore cheap insurance, not a performance strategy. `oqueue-buf` is the
strategy.

Two constraints:

- **NFR-42** — build with only cargo and a C compiler, except the FIPS build.
- **NFR-40** — Linux x86-64 and aarch64 are both first-class.

## Decision

**mimalloc, set in `bin/oqueue` and in no library crate.** Heap profiling is
`tikv-jemallocator` behind a **non-default** `heap-profiling` feature.

⚠️ **The allocator is a binary's choice, never a library's.** A library crate
that sets `#[global_allocator]` imposes it on every consumer — including tests,
benchmarks and any future embedder — and there is no way to opt out downstream.

### What it costs NFR-42

mimalloc is C, so `libmimalloc-sys` compiles it with `cc` at build time. That is
**within** NFR-42, which says "cargo and a C compiler" — it is not a new class of
requirement, and it is not the FIPS exception. ⚠️ Recorded here because
`AGENTS.md` forbids adding a dependency that pulls a C toolchain without a
reason. ⚠️ **And the 7% above is the wrong number for this trade**: it is
mimalloc against jemalloc, both of which need C. The alternative that needs
none is the system allocator, and doc 18 §3.5 gives no mimalloc-vs-glibc
figure — so the C dependency is justified by an argument, not a measurement,
and `M14`'s allocator benchmark is what settles it.

The cost is real though: a build host with no `cc` compiles the workspace up to
`bin/oqueue` and fails there. Everything under `crates/` stays pure Rust, so
`cargo test --workspace --exclude oqueue` needs no C at all.

### ⚠️ The jemalloc ARM page-size trap

This is why profiling is a feature and not the default, and it is a correctness
question rather than a footnote.

**jemalloc bakes the page size in at build time — the *build host's*.** Verified
in this workspace's own `tikv-jemalloc-sys` `config.log`: a native x86-64 build
probes, finds 4 KB, and writes `#define LG_PAGE 12`. ⚠️ **Native does not save
you.** The abort happens when the kernel a binary *runs on* has a larger page
than the one baked in, so an aarch64 artifact built natively on a 4 KB-page
runner still dies on a 64 KB-page kernel with
`<jemalloc>: Unsupported system page size`. Polars shipped exactly this
(pola-rs/polars#5654).

⚠️ **And this project builds natively** — `portability.md` rule 9 — so the trap
belongs to the pipeline this project has, not to one it decided against. NFR-40
makes aarch64 first-class, so a default that aborts on a legitimate deployment
host is not a default. `JEMALLOC_SYS_WITH_LG_PAGE=16` bakes 64 KB, which is safe
on both page sizes.

The fix is `JEMALLOC_SYS_WITH_LG_PAGE=16` at build time, which is a thing a
profiling build can be told and a shipping build should not have to be.

## Alternatives considered

**tikv-jemallocator as the default.** Rejected on both counts above: measurably
slower in the one benchmark doc 18 records, and carrying a startup-abort trap on
a first-class target. Kept for profiling because its `prof` support is genuinely
valuable — a broker's characteristic failure is "memory grows under a specific
consumer pattern", which is exactly what a heap profile answers.

**snmalloc.** ⚠️ Not rejected on merit, and doc 10 #30 is explicit that this is
the open part: its message-passing free design matches this workspace's
allocation shape exactly — one thread allocates a buffer, another frees it —
and it is **unbenchmarked here**. It is not chosen now because choosing it would
mean preferring an argument to a measurement, and doc 10 #30 says "worth one
measurement". ⚠️ **That measurement is `M14`'s, and `M14.md` now carries it** — recorded
there in this commit, because an obligation that lives only in an ADR is one
the receiving milestone closes green without.

**The system allocator.** Rejected as the default and worth keeping reachable:
it is the only option with no build-time C at all, so it is what a host without
`cc` would need. Not wired today because nothing needs it and an unused feature
is an untested one — ⚠️ if NFR-42's clean-container test ever runs without a C
compiler, this is the gap it will find.

**Setting the allocator in `oqueue-broker` instead.** Rejected: it is a library.
See the Decision.

## Consequences

**Easy.** ~7% on residual small-object traffic for one line, and a heap profiler
one feature flag away when a memory-growth bug appears.

**Hard.** A C compiler is now required to build the binary, and `bin/oqueue`'s
dependency tree gained a `cc` build script — which doc 18 §3.7.2 names as a real
contributor to `target/` size.

⚠️ **What has no gate.**

- **That no library crate sets a global allocator.** Nothing checks it;
  `check-layering.sh` reads dependency names, not attributes. Today it is one
  `grep` away from being true, and it is stated in `bin/oqueue`'s AGENTS.md so a
  reviewer has something to check against.
- **That the `heap-profiling` build *runs* anywhere.** ⚠️ It is compiled by
  every gate — `check-crate.sh` lints with `--all-features` — and executed by
  none. `M0.13` changed that script to run only the shipping configuration,
  because with `--all-features` the gate ran a jemalloc-linked binary and never
  ran the mimalloc one that ships.

  ⚠️ **It did *not* change for the reason first written down.** The first
  version said `--all-features` would abort the gate on a 64 KB-page runner.
  That is wrong: the probe bakes in the *build host's* page size, so a gate
  building and running on one machine never mismatches itself. The trap is a
  build host and a deployment host with different page sizes — `M13`'s problem,
  and `M13.md` now carries it.

  What *was* verified here by hand, on x86-64: the feature links jemalloc and
  not mimalloc (63 prof symbols, 0 mimalloc), the default build is the reverse,
  and `_RJEM_MALLOC_CONF=prof:true,prof_prefix:...,prof_final:true` writes a
  ~3.5 KB heap dump. ⚠️ **The variable is `_RJEM_MALLOC_CONF`, not
  `MALLOC_CONF`** — `tikv-jemalloc-sys` prefixes its symbols by default, so the
  unprefixed name is read by nothing and fails silently. ⚠️ **That verification is the reason the feature
  works at all** — review found that `["dep:tikv-jemallocator"]` alone swaps the
  allocator while leaving profiling *compiled out*, so `prof:true` was rejected
  as an invalid conf pair and no dump was ever written. A feature named
  `heap-profiling` that does not profile is the shape to watch for.
  `tikv-jemallocator/profiling` is the half that matters.
- **That mimalloc is still the right choice.** The 7% is one published
  benchmark on someone else's workload. `M14` measures this workspace's.
