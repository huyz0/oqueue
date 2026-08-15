# `oqueue-crypto`

## What is it?

AEAD, envelope encryption, the DEK cache, and nonce construction.

## Why does it exist?

Because BYOK is a per-topic opt-in and server-side encryption cannot express it: one object carries many tenants' data and can only carry one SSE-KMS key. So encryption is broker-side, and this is where it lives.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.

## Downstream

`oqueue-broker`, and through it `bin/oqueue`.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## Invariants

| Must stay true | Held by |
|---|---|
| A nonce is never random | review; `security.md` |
| Key material is zeroized on drop | review — `security.md` rule 8 |
| No key material reaches a formatted string | `oqueue_core::Redacted`, and `check-secrets.sh` once `M8` writes it |

## Notes for whoever touches this

- ⚠️ **Nonces are constructed, never random.** `security.md` — a repeated nonce under the same key is a total loss of confidentiality for both messages.
- **Key material is zeroized on drop** (`security.md` rule 8) and never reaches a formatted string. `oqueue_core::Redacted` gives the formatting half; zeroization is this crate's.
- ⚠️ **This crate is absent from doc 19's ten-crate layout** and exists only in `architecture.md`. It is kept because doc 22's envelope design needs somewhere to live that is not the broker; dropping it would be an ADR, not a tidy-up.
