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

## What has no gate

**Whether a benchmark represents a real workload.** A fast benchmark on an
unrepresentative input is worse than none, because it produces confidence.
Benchmark inputs are reviewed like code and their provenance is recorded.

## See also

- Full methodology, with evidence: [docs/researches/18](../../researches/18-rust-performance-methodology.md)
- Latency budget and what is achievable: [docs/researches/17](../../researches/17-latency-budget.md)
- API cost model: [docs/researches/12](../../researches/12-object-discovery-and-api-cost.md)
