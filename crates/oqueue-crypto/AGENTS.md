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

## ⚠️ The algorithm is read, never assumed

`region::open` dispatches on the [`RegionAlg`] its caller read out of the
region header. ⚠️ **Do not add a convenience that decrypts without one** — the
whole reason the header carries the field is that `M13`'s FIPS build must read
objects this build wrote and vice versa (ADR-0050 point 7, ADR-0012), and an
API that assumes AES-GCM makes that unprovable. `RegionAlg::None` is refused by
both `seal` and `open`; it is not an algorithm.

## ⚠️ Randomness comes through the `Entropy` seam, never from the OS directly

`M8.5`. A fresh DEK's 32 bytes come from `entropy::Entropy`; `OsEntropy` is the
real implementation and a composition root is the only place that should build
one. ⚠️ **Do not widen the seam to "n random bytes."** A general randomness API
in scope is how a *random nonce* gets written, and `ADR-0050` point 2 makes
nonces constructed precisely so that cannot happen. `FakeEntropy` is a counter
and says so: it is a test double, and a DEK from it is guessable.

## ⚠️ Both caches hold a `std::sync::Mutex` and await nothing under it

`async-concurrency.md` rules 6 and 8. The one `.await` on each path is the KMS
call, and it happens with no lock held — which means two tasks can race and
both call the KMS. Both caches resolve that by **keeping the incumbent**, so
the cost is one wasted call and never two live DEKs for a topic. Serializing
instead would hold a lock across a network round trip.

⚠️ The caches hand out `&Dek` through a `FnOnce`, not by return, because `Dek`
is deliberately not `Clone`. Do not `.await` inside one of those closures, and
⚠️ **do not call back into the cache from one either** — the lock is held for
the length of the call and `std::sync::Mutex` is not reentrant, so every
accessor (`sealed_bytes`, `holds_live_dek`, `live_topics`, `live_dek_is`,
`len`, `is_empty`) deadlocks that task permanently. The `.await` rule is held by
the type system; this one is not.

## ⚠️ The read cache is bounded by its capacity, not by its TTL

There is no sweep here and no timer, so an expired entry nobody looks up again
is reached by nothing — that was a real leak of plaintext key material until
`UNWRAPPED_DEK_CACHE_ENTRIES` and `Entries::make_room` landed. ⚠️ **Do not
"simplify" the eviction away**, and do not add a background sweeper to replace
it: this crate compiles no runtime, which is what lets `M0.16` measure NFR-56's
floor. What holds is stated precisely in the module doc — at most N entries,
each at most a TTL old *as of the last insert*.

## ⚠️ Provider adapters stop at encrypt/decrypt

`M8.7`'s [`providers`](src/providers.rs) module maps AWS KMS `Encrypt`/`Decrypt`
and GCP Cloud KMS `encrypt`/`decrypt` to the one `KeyProvider` seam. It owns no
vendor SDK and makes no network call; in-process API simulations prove the
mapping here, while real cloud round trips belong to M15. Do not add
`generate_data_key`: AWS has it, GCP does not, and the seam deliberately
generates DEKs locally before wrapping them.

## ⚠️ The rest of this crate is still being filled in

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does; `M0.11` added the refusing no-op, `M8.3` the region AEAD, and `M8.5` the
write- and read-side DEK caches with the entropy seam. The KMS providers are
the rest of `M8`'s — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
