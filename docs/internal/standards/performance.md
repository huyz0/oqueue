# Performance

Rules for measuring and optimizing. The methodology and its evidence are in
[docs/researches/18](../../researches/18-rust-performance-methodology.md); this
file is what binds.

## Measure before optimizing, and measure the right thing

1. **No optimization lands without a benchmark showing it helps.** The number
   goes in the commit message. → review
2. **Three benchmark suites, and only one gates merges:**

   | Suite | Harness | Gates? |
   |---|---|---|
   | `bench-micro` — pure CPU leaves: codec, varint, CRC, index lookup | **gungraun** (instruction counts) | ✅ every commit |
   | `bench-macro` — end-to-end produce/fetch, batching, multi-threaded | criterion, quiet hardware | ❌ report only |
   | `bench-simd` — anything with runtime ISA dispatch | criterion, real ISA | ❌ report only |

   ⚠️ **Gating on a metric that cannot see the bottleneck trains people to
   ignore alerts.** Instruction counts cannot see I/O, lock contention, or
   memory bandwidth, and Valgrind cannot execute AVX-512 or SVE at all.

3. **The instruction-count gate runs on x86-64 Linux CI only.** Counts are
   architecture-specific, so a number from another platform is not comparable to
   the baseline. Local wall-time comparison is for finding regressions, never
   for adjudicating them.
4. ⚠️ **Assert the dispatch path inside any benchmark whose callee does runtime
   feature detection.** Under Valgrind, `is_x86_feature_detected!` returns false,
   so a dispatcher silently selects the fallback and the benchmark measures code
   production never runs. → the benchmark itself fails on mismatch
5. **Use `Ir` as the gate.** Estimated Cycles is a fixed-constant model of a
   mid-2000s memory hierarchy; treat it as a hint that something memory-shaped
   moved, never as a performance number.

## Where performance actually comes from

Ordered by observed leverage. **Exhaust the earlier rows before the later ones.**

6. **Architecture first.** Not decoding varints on ingest, deriving a CRC with
   `crc32c_combine` instead of recomputing it, batching index lookups, pooling
   buffers. These dwarf everything below.
7. **Data layout second.** Cache-friendly index structures beat wider vectors:
   layout and batching account for most of the gain in search, not vector width.
8. **Build configuration third.** Thin LTO is mandatory in a multi-crate
   workspace — crate boundaries are optimization barriers by default (see
   `build.md`).
9. **SIMD fourth, and only through the decision procedure** in doc 18 §4.5. Stop
   at the first "no": profiled above ~5% of CPU, compute-bound, not already
   autovectorized, no maintained crate does it, buffer large enough to amortize
   dispatch.
10. **`unsafe` last, and usually not at all.** Bounds-check elimination is worth
    1–3%. Try the safe techniques in doc 18 §5.4 first and say in the PR which
    ones failed and why. → the six conditions in `security.md` rule 19

## Rules that prevent silent loss

11. **The benchmark profile differs from the shipping profile in exactly one
    way**: `debug = true`. Every other divergence measures a different program.
    ⚠️ `debug-assertions` accidentally left on is the most common way a Rust
    benchmark is invalidated.
12. **`cargo bench` inherits `release` unless pinned.** Pin it, or you benchmark
    codegen that is never shipped.
13. **A CRC implementation must be CRC-32C (Castagnoli).** ⚠️ `crc32fast` is
    CRC-32/IEEE — the wrong polynomial for Kafka, with no compile error and no
    runtime error, just batches every client rejects. → a known-answer test
    against published vectors
14. **A slow test is an architecture signal.** Sans-I/O logic tests in
    sub-milliseconds; one that takes 500 ms has acquired I/O it should not have.
    → the per-test time threshold, which is a layering gate wearing a
    performance gate's clothes

## Budgets

15. **Performance requirements are numbers with gates**, not aspirations. NFR-1
    through NFR-4 in [requirements.md](../product/requirements.md) are the
    latency budget; NFR-30 through NFR-33 are the cost budget.
16. **The pre-commit suite has a time budget enforced by a script**, not by a
    comment. → `check-budget.sh`. ⚠️ A budget stated only in prose is a
    preference.
17. ⚠️ **Cost is a performance property here.** An object-storage API call is a
    latency cost *and* a money cost, and LIST is priced at 12–38× a GET. A
    change that adds an API call to a hot path is a performance regression even
    if it is fast. → NFR-30's zero-LIST gate

## Hot-path benchmarks

18. **Every hot path has a benchmark**, added with the code rather than after
    it. The hot paths, and each one's benchmark obligation:

    | Path | Benchmark |
    |---|---|
    | RecordBatch encode / decode | `bench-micro`, gated |
    | CRC-32C over representative sizes | `bench-micro`, gated + known-answer test |
    | Varint decode (and the paths that avoid it) | `bench-micro`, gated |
    | Offset→object index lookup | `bench-micro`, gated |
    | Buffer allocation and pooling | `bench-micro` + heap profile |
    | Produce path end to end | `bench-macro`, report only |
    | Fetch: tail (cached) and cold (ranged GET) | `bench-macro`, report only |
    | Compaction throughput | `bench-macro`, report only |

19. **A hot path without a benchmark is an unmeasured claim.** → a gate
    asserting each named path has one, so the list above cannot silently rot.

## Profiling: on demand, never gated

⚠️ **These are tools, not gates.** They are slow, they need a quiet machine, and
several produce output that requires judgement to read. Gating on them would
either slow the loop to a crawl or produce alerts nobody trusts. They must be
**runnable at any moment** without ceremony, which is the whole point.

20. **Every profiling mode is a one-line command**, documented in
    `scripts/profile.sh`. A profiler that takes twenty minutes to set up is a
    profiler nobody runs.

| Want to know | Tool | Command |
|---|---|---|
| Instructions retired, per function | Callgrind via **gungraun** | `scripts/profile.sh instructions <bench>` |
| Where wall time goes | `perf` + flamegraph | `scripts/profile.sh flame <bench>` |
| Heap: what allocated, where, how long | **DHAT** | `scripts/profile.sh heap <bench>` |
| Peak memory over time | **Massif** | `scripts/profile.sh massif <bench>` |
| Cache misses and branch misprediction | **Cachegrind** | `scripts/profile.sh cache <bench>` |
| Allocation count and size distribution | allocator stats | `scripts/profile.sh alloc <bench>` |
| Which mutants survive here | cargo-mutants, scoped | `scripts/mutants.sh <crate>` |
| Full mutation run, sharded | cargo-mutants | `scripts/mutants.sh` |

21. **Memory is a first-class concern, not an afterthought.** A broker holding
    buffers for many partitions fails on allocation behaviour long before it
    fails on CPU. Heap profiling belongs in the routine, and **allocation count
    is often a better regression signal than bytes** — it catches a new copy
    that a peak-memory number hides.
22. **Profile before optimizing, and keep the profile.** Attach it to the task.
    A performance claim with no profile behind it is a guess that has been
    written down.
23. ⚠️ **Valgrind-based tools cannot execute AVX-512 or SVE**, and under
    Valgrind runtime feature detection reports false. For any SIMD-dispatching
    path, use `perf` on real hardware — the Valgrind number describes code that
    does not ship. → rule 4

## What has no gate

**Whether a benchmark represents a real workload.** A fast benchmark on an
unrepresentative input is worse than none, because it produces confidence.
Benchmark inputs are reviewed like code and their provenance is recorded.

## See also

- Full methodology, with evidence: [docs/researches/18](../../researches/18-rust-performance-methodology.md)
- Latency budget and what is achievable: [docs/researches/17](../../researches/17-latency-budget.md)
- API cost model: [docs/researches/12](../../researches/12-object-discovery-and-api-cost.md)
