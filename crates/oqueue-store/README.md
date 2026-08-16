# `oqueue-store`

## What is it?

`ObjectStore` implementations: S3, GCS, and an in-memory backend.

## Why does it exist?

Because the seam is in `oqueue-core` and the concrete backends must live somewhere that is not `oqueue-core`. This crate is where a vendor SDK is allowed to appear.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.
- `object_store` — the S3/GCS client library ADR-0008 chose; this crate's backends are thin adapters over it.
- `reqwest` — feature-only, not called directly; steers Cargo's feature unification for the shared HTTP client `object_store` also depends on, away from `aws-lc-rs` (ADR-0012).
- `rustls` — feature-only for the same reason as `reqwest`, plus the crypto-provider install call `tls.rs` makes at runtime.

## Downstream

`oqueue-broker`, and through it `bin/oqueue`.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## Invariants

| Must stay true | Held by |
|---|---|
| Every backend passes the same conformance suite | `M1`'s suite, against the fake and S3 (MinIO). ⚠️ **Not yet GCS** — ADR-0014: no available emulator round-trips `object_store`'s GCS requests yet, so `GcsStore` is T0-verified only |
| Conditional-write semantics match real S3 | ⚠️ **unverified until it runs against real S3** — doc 10 #33 |
| Conditional-write semantics match real GCS | ⚠️ **unverified against anything live at all**, not even an emulator — ADR-0014, stronger than the S3 row above |

## Notes for whoever touches this

- ⚠️ **The in-memory backend here is a real implementation, not a fake.** `testing.md` rule 6 and `contracts.md` rule 9: the *fake* lives beside the trait in `oqueue-core`. Two things that look alike and are not — the fake is for downstream unit tests, this is a backend that must pass the same conformance suite as S3.
- ⚠️ **Conditional-write semantics are the highest-risk surface in the project** (doc 10 #33). If this backend and the fake disagree about a failed precondition, the result is an architectural error, not a test gap.
