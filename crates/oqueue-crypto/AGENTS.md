# `oqueue-crypto` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-crypto` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ **Nonces are constructed, never random.** `security.md` — a repeated nonce under the same key is a total loss of confidentiality for both messages.
2. **Key material is zeroized on drop** (`security.md` rule 8) and never reaches a formatted string. `oqueue_core::Redacted` gives the formatting half; zeroization is this crate's.
3. ⚠️ **This crate is absent from doc 19's ten-crate layout** and exists only in `architecture.md`. It is kept because doc 22's envelope design needs somewhere to live that is not the broker; dropping it would be an ADR, not a tidy-up.

## ⚠️ `NoOpKeyProvider` exists and **refuses**

`M0.11` added it, and ADR-0006 records why it returns `EncryptionDisabled`
rather than passing the plaintext DEK through: an identity provider would write
a **plaintext data encryption key** into object storage beside the data it
protects, looking exactly like a successful wrap.

⚠️ **Do not add a second, identity-style provider beside it.** `M8.md`'s plan
predates the decision and asks for a "no-op" in the natural reading; the natural
reading is the rejected alternative.

## ⚠️ The rest of this crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
