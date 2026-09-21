# `oqueue-crypto`

## What is it?

AEAD, envelope encryption, the DEK cache, and nonce construction.

## Why does it exist?

Because BYOK is a per-topic opt-in and server-side encryption cannot express it: one object carries many tenants' data and can only carry one SSE-KMS key. So encryption is broker-side, and this is where it lives.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.
- `aes-gcm` — RustCrypto's pure-Rust AES-256-GCM, the AEAD `region::seal` and
  `region::open` use (`M8.3`). Taken **without its `getrandom` feature**, so
  `Aes256Gcm::generate_nonce` and every other random-nonce constructor stay out
  of scope; no build script, so the default build's toolchain stays cargo plus
  a C compiler (NFR-42, ADR-0012).
- `getrandom` — the OS CSPRNG behind `entropy::OsEntropy`, the source of a
  fresh DEK's 32 bytes (`M8.5`). ⚠️ **This is a direct dependency now, so "the
  crate cannot reach a random source" is no longer the thing keeping nonces
  constructed** — what keeps them constructed is `oqueue_core::Nonce`, which
  has no constructor from bytes at all, and the `Entropy` seam being exactly
  32 bytes wide with no second caller. No build script, no C toolchain, and
  already in `Cargo.lock` transitively before this edge existed.
- `zeroize` — wiping the scratch buffer a DEK is minted in and the plaintext
  copy handed to `KeyProvider::wrap`; `oqueue_core::Redacted` has no `Drop` of
  its own, so that is the owner's job (`security.md` rule 8).
- `tracing` — emitting bounded child spans for KMS wrap and unwrap round trips;
  the broker owns subscriber and exporter configuration (`M12.12`, ADR-0067).

`providers` contains the thin AWS KMS and GCP Cloud KMS adapters. They expose
only each API's encrypt/decrypt operations and implement the same
`oqueue_core::KeyProvider` seam; M8 tests them against in-process API
simulations, while real cloud credentials and round trips remain M15's.

## Downstream

`oqueue-broker`, and through it `bin/oqueue`.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## Invariants

| Must stay true | Held by |
|---|---|
| A nonce is never random | review; `security.md` — and by construction: `oqueue_core::Nonce` has no constructor from raw bytes, and `aes-gcm` is taken without its `getrandom` feature so no random-nonce constructor is in scope. ⚠️ Since `M8.5` the crate *does* depend on `getrandom` directly, behind the `Entropy` seam; the seam is 32 bytes wide and has one caller, which is what keeps it from becoming a nonce source |
| Key material is zeroized on drop | review — `security.md` rule 8 |
| No key material reaches a formatted string | `oqueue_core::Redacted`, and `check-secrets.sh` once `M8` writes it |

## Notes for whoever touches this

- **`region::seal`/`region::open` are the AEAD** (`M8.3`): AES-256-GCM over
  one region, with the algorithm taken from the region header at open time and
  never assumed. The associated data binds the region's identity — topic,
  partition, region index and algorithm code — and deliberately **not** the
  object key or byte range; `region.rs`'s own docs say precisely what that does
  and does not buy. Sealing costs exactly sixteen bytes, the untruncated GCM
  tag. ⚠️ `M13`'s FIPS build swaps the *implementation* behind
  `RegionAlg::Aes256Gcm`, never the format (ADR-0050 point 7, ADR-0012).

- **`NoOpKeyProvider` refuses, and is the other thing here.** `M0.11`,
  ADR-0006. ⚠️ It is *production* code for the unencrypted path, not a fake —
  which is why it lives here and the fake lives beside the trait in
  `oqueue-core`. It returns `EncryptionDisabled` rather than passing the
  plaintext DEK through, because an identity provider would write a plaintext
  data encryption key into object storage looking exactly like a real wrap.

- **The DEK caches are what make `NFR-33` true** (`M8.5`): `dek_cache` holds one
  live DEK per topic with the `WrappedKey` the KMS returned, rotating at
  whichever comes first of `DEK_MAX_SEALED_BYTES` (64 GiB) or `DEK_MAX_AGE_MS`
  (7 days) — `ADR-0050` point 3, both pinned in `check-drift.sh`. `unwrap_cache`
  holds unwrapped DEKs keyed by `(key id, wrapped blob)` — ⚠️ **never by topic**,
  because a reader meets every DEK the topic ever rotated through — for
  `UNWRAPPED_DEK_TTL_MS`, at most `UNWRAPPED_DEK_CACHE_ENTRIES` of them.
  ⚠️ That TTL is where a KMS outage becomes visible: reads of BYOK topics fail
  once an entry ages out (`ADR-0050`'s last consequence). ⚠️ **The TTL alone
  bounds nothing about memory** — there is no sweep and no timer here, so an
  entry nobody looks up again is reached only by the eviction an *insert*
  performs; the capacity is what makes the memory bound true, and the module
  doc states exactly what holds and between which events.

- **A fresh DEK's randomness comes through `entropy::Entropy`** (`M8.5`), never
  from a direct OS call in library logic. `OsEntropy` is for a composition root;
  `FakeEntropy` is a counter and is not entropy.

- **The provider adapters only wrap and unwrap** (`M8.7`, ADR-0050 point 6).
  Neither adapter exposes `generate_data_key`: AWS has that operation, but GCP
  does not, so the portable seam stays at the intersection.

- ⚠️ **Nonces are constructed, never random.** `security.md` — a repeated nonce under the same key is a total loss of confidentiality for both messages.
- **Key material is zeroized on drop** (`security.md` rule 8) and never reaches a formatted string. `oqueue_core::Redacted` gives the formatting half; zeroization is this crate's.
- ⚠️ **This crate is absent from doc 19's ten-crate layout** and exists only in `architecture.md`. It is kept because doc 22's envelope design needs somewhere to live that is not the broker; dropping it would be an ADR, not a tidy-up.
