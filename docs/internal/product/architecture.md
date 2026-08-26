---
title: "Architecture"
description: >
  Read before adding a crate or a trait seam, or when unsure which crate something belongs in.
tags: [product, crates, seams, layering]
---

# Architecture

The crate map and the dependency rule.

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

Enforced by `scripts/check-layering.sh` (M-1.8), which `M0.21` widened to the
three other properties it can read off the same manifests — `lints.workspace`,
no member `[profile]`, and `overflow-checks` in the root release profile.
`[dev-dependencies]` are exempt,
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
| `oqueue-index` | The materialization a broker serves reads from, and what fills it | forbid |
| `oqueue-store` | `ObjectStore` implementations: S3, GCS (⚠️ **not** in-memory — that is `oqueue-core`'s `FakeObjectStore`; `M1.37`) | forbid |
| `oqueue-coordinator` | Metadata, offset sequencing, recovery | forbid |
| `oqueue-crypto` | AEAD, envelope encryption, DEK cache, nonce construction | forbid |
| `oqueue-compact` | Compaction planning and execution | forbid |
| `oqueue-broker` | **Composer.** The I/O shell, generic over its seams | forbid |
| `oqueue-testkit` | Harness and generators. **Dev-only.** ⚠️ **No fakes** — `contracts.md` rules 9 and 11 put every fake beside its trait in `oqueue-core`, so a downstream crate is testable without depending on the testkit. Corrected in `M0.8`; this cell said "Fakes, generators, harness" | forbid |
| `bin/oqueue` | Composition root — where concrete types are chosen | forbid |

The `unsafe` column is a three-crate budget, and it falls out of the split for
free: the crates that need it are exactly the leaf primitives. A fourth requires
a recorded decision. See
[docs/researches/18](../../researches/18-rust-performance-methodology.md) §5.7.

⚠️ **Watch `oqueue-core`'s size.** Every crate depends on it, so every change to
it rebuilds the workspace. A comparable service holds around a dozen traits
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
  ⚠️ **Carries no `list()`** — ADR-0009 puts listing behind a separate,
  not-yet-built `MaintenanceStore` seam so "never LIST on the read path"
  (NFR-30) is a property nothing holding only an `ObjectStore` can violate,
  rather than a runtime gate someone has to remember to keep passing.
- **`KeyProvider`** — wrap and unwrap only. Deliberately *not* "generate a data
  key": GCP Cloud KMS has no `GenerateDataKey` equivalent, so the seam is the
  intersection of what AWS and GCP both offer, and DEKs are generated locally.
  See [docs/researches/22](../../researches/22-encryption-byok-and-fips.md) §4.

⚠️ **The `ObjectStore` fake is the highest-risk component in the project.**
Conditional writes (`If-Match`/`If-None-Match`) are load-bearing for the whole
CAS design. If the fake and MinIO are both more permissive than real S3, the
result is an architectural error rather than a test gap. Assertions are written
to the documented semantics in
[docs/researches/04](../../researches/04-object-storage-s3-gcs.md) §6, and
**conditional-write behaviour stays marked unverified until it has run against
real S3.** Open question #33.

## Encryption

Per-topic keys and multi-topic object batching collide: an object bundles many
tenants precisely because that is what makes the cost model work, and records
under different keys cannot share an encryption context.

**BYOK is expected on ~10,000 topics out of 1M–100M, and that rarity decides the
resolution: segregate rather than complicate.** An object is *either* a
default-key object — the >99% path, unchanged — *or* a BYOK object holding only
regions whose topics share one KEK. Regions inside a BYOK object are still
sealed per topic, so a customer's topics stay isolated from each other.

⚠️ The rejected alternative was sealing every region of every object
independently. It works, but pays object-format complexity on 100% of traffic to
serve under 1% of it.

⚠️ **BYOK topics accept worse batching efficiency** as the price, since they can
only batch within a key domain. That is the customer's trade to make, and it
should be stated rather than absorbed.

`oqueue-crypto` sits between the codec and the store; `oqueue-store` stays
unaware that bytes are encrypted, which keeps the object-storage conformance
suite independent of encryption. KMS is on the DEK-rotation path and **never on
the per-batch path** — cached DEKs put KMS at roughly 3 ops/sec against a
5,500/sec quota, where an uncached design would throttle at modest load.

Full derivation in
[docs/researches/22](../../researches/22-encryption-byok-and-fips.md).

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
