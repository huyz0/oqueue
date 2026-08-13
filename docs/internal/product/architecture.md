# Architecture

The crate map and the dependency rule. Written fresh for oqueue rather than
ported, because the seams here are `ObjectStore` and `Clock`, not a proxy's.

## The dependency rule

**Every crate depends on `oqueue-core` and on nothing else in the workspace**,
with two named exceptions: `oqueue-broker` composes, and `bin/oqueue` composes
everything. `oqueue-testkit` is dev-only and must never appear in a runtime
`[dependencies]` section.

This is the rule that lets work proceed on several crates without conflict, and
it is what keeps the build DAG **wide and shallow** — depth 2 for every library
but the composer. Depth, not crate count, sets the floor on build time, because
a dependent starts as soon as its dependency's `.rmeta` exists. Proc-macro
crates and build scripts break that pipelining and are therefore kept few, tiny,
and at the root. See
[docs/researches/19](../../researches/19-workspace-engineering.md) §1–2.

Enforced by `scripts/check-layering.sh` (M-1.8). `[dev-dependencies]` are exempt,
because a test may compose and because Cargo permits a dev-dependency cycle,
which is what makes a shared testkit usable at all.

## The crates

```
                        ┌─ buf ──────┐
                        ├─ codec ────┤
   core ◄───────────────┼─ checksum ─┼──── broker ──── bin/oqueue
   (no workspace deps;  ├─ index ────┤   (composer)     (composer)
    every trait seam)   ├─ store ────┤
                        ├─ coordinator
                        └─ compact ──┘
```

| Crate | Holds | `unsafe` |
|---|---|---|
| `oqueue-core` | Types, IDs, errors, and **every trait seam**. No I/O, no async runtime, no dependencies on anything else here. | forbid |
| `oqueue-buf` | Buffer primitives, refcounted slices, pooling | **allowed** |
| `oqueue-codec` | Kafka wire protocol, RecordBatch encode/decode | **allowed** |
| `oqueue-checksum` | CRC-32C | **allowed** |
| `oqueue-index` | Offset→object index and its search | forbid |
| `oqueue-store` | `ObjectStore` implementations: S3, GCS, in-memory | forbid |
| `oqueue-coordinator` | Metadata, offset sequencing, recovery | forbid |
| `oqueue-compact` | Compaction planning and execution | forbid |
| `oqueue-broker` | **Composer.** The I/O shell, generic over its seams | forbid |
| `oqueue-testkit` | Fakes, generators, harness. **Dev-only.** | forbid |
| `bin/oqueue` | Composition root — where concrete types are chosen | forbid |

The `unsafe` column is a three-crate budget, and it falls out of the split for
free: the crates that need it are exactly the leaf primitives. A fourth requires
a recorded decision. See
[docs/researches/18](../../researches/18-rust-performance-methodology.md) §5.7.

⚠️ **Watch `oqueue-core`'s size.** Every crate depends on it, so every change to
it rebuilds the workspace. The precedent this is modelled on holds 12 traits
comfortably. If ours approaches ~40, split `core-types` (rarely changes) from
`core-traits`. Tracked as open question #34.

## The seams

`oqueue-core` defines traits; other crates implement them. Dependency inversion
is what makes the star topology possible and what makes every crate testable
without constructing the system.

The two that carry the most weight:

- **`Clock`** — nothing else reads the real time. Removes the largest single
  class of flaky test by construction.
- **`ObjectStore`** — nothing else talks to S3 or GCS. This extends the
  sans-I/O rule beyond its usual form and is what allows latency, 503s,
  conditional-write races, and partial failures to be injected deterministically.

⚠️ **The `ObjectStore` fake is the highest-risk component in the project.**
Conditional writes (`If-Match`/`If-None-Match`) are load-bearing for the whole
CAS design. If the fake and MinIO are both more permissive than real S3, the
result is an architectural error rather than a test gap. Assertions are written
to the documented semantics in
[docs/researches/04](../../researches/04-object-storage-s3-gcs.md) §6, and
**conditional-write behaviour stays marked unverified until it has run against
real S3.** Open question #33.

## Sans-I/O

Business logic touches no socket, no clock, and no object store. The I/O shell
is generic over its bounds and lives entirely in `oqueue-broker`; `bin/oqueue`
chooses the concrete types. A concrete socket type named inside a library crate
is a violation; a generic bound is not.

This one constraint delivers three things at once: crates testable in isolation,
determinism (no wall clock, no scheduler, no network), and mutation-testability,
since pure state machines are what mutation testing works on.

A corollary worth stating because it is also a gate: **if business logic is
sans-I/O, its tests are sub-millisecond.** A test that suddenly takes 500 ms has
acquired I/O somewhere it should not have, so the per-test time threshold is a
second detector for this rule.

Enforced by `scripts/check-sans-io.sh` (M-1.8).

## Build and release

One binary per `(os, arch)` at `x86-64-v2` / aarch64 `+lse,+crc`, with everything
above the baseline behind runtime dispatch — required anyway for CRC-32C, where
the spread is roughly 12 GB/s to 97 GB/s. glibc floor pinned at 2.28 via
`cargo-zigbuild`. Both Linux architectures are first-class; macOS is a
development platform and not a release target. See
[docs/researches/20](../../researches/20-build-and-release-portability.md).
