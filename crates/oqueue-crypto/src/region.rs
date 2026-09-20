//! Sealing and opening **one region** of a bundled object.
//!
//! `ADR-0050` point 1: a sealed region carries `{key_id, wrapped_dek, nonce,
//! alg}`, and *"`alg` … is read at open time, never assumed"*. This module is
//! the half of that sentence the compiler can hold: [`open`] takes the
//! [`RegionAlg`] that came out of the header and dispatches on it, and there
//! is no path through it that decrypts without having been told what with.
//!
//! # ⚠️ The algorithm is an input, not a constant
//!
//! It would be shorter to call AES-256-GCM directly and keep `alg` as a value
//! nobody reads. That is exactly the shape `FR-43` exists to forbid: `M13`'s
//! FIPS build swaps the AES-GCM *implementation* — `aws-lc-rs` FIPS in place of
//! this crate's pure-Rust one (`ADR-0012`) — and both builds must read each
//! other's objects. Swapping an implementation behind one algorithm code works
//! only if the code is what selects it. [`RegionAlg::Aes256Gcm`] therefore
//! names a **format**, not a library, and `M13` changes neither the code nor
//! the bytes on either side of it (`ADR-0050` point 7).

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead as _, KeyInit as _, Payload};
use oqueue_core::{Dek, Error, KeyId, Nonce, ParsedNonce, PartitionId, RegionAlg, Result, TopicId};

/// How many bytes sealing adds to a region: AES-GCM's 128-bit tag, appended.
///
/// ⚠️ **Fixed by the construction, not chosen.** GCM's tag may be truncated in
/// the standard; this format does not truncate it, because a shortened tag is
/// a weaker forgery bound and the saving is sixteen bytes on a region whose
/// size is measured in megabytes. So a sealed region is always exactly
/// `plaintext.len() + TAG_BYTES` long, and the nonce lives in the header
/// rather than in these bytes.
pub const TAG_BYTES: usize = 16;

/// What a sealed region is bound to: everything that says *which* region it
/// is.
///
/// # ⚠️ What is bound, and what is not
///
/// The AEAD's associated data is this struct's encoding, and it is the whole
/// of what a tag proves about a region's *identity*. Bound:
///
/// - the **topic name**, length-prefixed — so a region cannot be relabelled as
///   another topic's, which under BYOK is another tenant's;
/// - the **partition** — so region *i* of topic `t` cannot be served as
///   partition *j* of the same topic, which would reorder a log;
/// - the **region index** within the object — so two regions of one object
///   cannot be swapped;
/// - the **algorithm code** — so a header edited to name a different algorithm
///   fails to open rather than being decoded under the wrong one;
/// - the **key id** the footer names (`M8.4`, carrying `M8.12`'s obligation) —
///   so a region cannot be relabelled as sealed under a different KEK, which
///   under BYOK is a different tenant's key domain. ⚠️ The DEK already fails a
///   forged key id in practice, because the wrong key gives the wrong tag; the
///   binding is what makes that a *guaranteed* refusal rather than a
///   consequence of the two always disagreeing.
///
/// Bound without being in this struct:
///
/// - the **nonce**. It is an input to AES-GCM itself, not associated data, so
///   editing the twelve bytes the footer carries changes the keystream and the
///   tag fails. ⚠️ Adding it here as well would be a second copy of a binding
///   the construction already gives, and a reader would have to check which of
///   the two a mismatch came from.
///
/// Not bound, and each is deliberate:
///
/// - the **object key**. A region is moved between objects by compaction
///   (`M5`) *without* re-sealing when its key domain is unchanged, so binding
///   the object's name would make every rewrite a decrypt-and-reseal. What
///   stops a region being moved into a *hostile* object is the nonce rather
///   than the AAD: an (epoch, object sequence, region index) triple is unique
///   per writer, so a region lifted into another object keeps a nonce that
///   does not match that object's other regions, and the manifest a reader
///   trusts is itself authenticated. ⚠️ **This is the weakest point of the
///   binding and is written down rather than implied**: a region *is*
///   cryptographically movable between objects of the same topic, partition
///   and index. `M8.4`, which puts the envelope in the footer, is where the
///   region header's own bytes come under the tag;
/// - the **byte range**. It is a property of where the region landed, not of
///   what it says, and compaction changes it without changing the records;
/// - the **record count**, for the same reason the byte range is not — it is
///   the footer's claim about the region, and `M8.4` is where the footer's
///   claims are covered;
/// - the **wrapped DEK's bytes**. ⚠️ `ADR-0050` point 4's lazy re-wrap is why,
///   and `M8.4` re-examined the argument rather than inheriting it: a DEK is
///   re-wrapped under the current KEK version when it next rotates or when
///   compaction rewrites its data, so the wrapped bytes legitimately change
///   for an unchanged key and an unchanged region — binding them would turn
///   every re-wrap into a decrypt-and-reseal of the data, which is exactly the
///   eager rewrite that point forbids. The argument still holds, and what it
///   concedes is bounded: substituting these bytes yields an unwrap failure at
///   the KMS or a different DEK, and a different DEK fails the tag. So the
///   reachable harm is a denial of service on one region, never a region
///   opening as something it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionAad<'a> {
    /// The topic whose records the region holds.
    pub topic: &'a TopicId,
    /// The partition whose records the region holds.
    pub partition: PartitionId,
    /// The region's position in the object, the same index its nonce carries.
    pub region_index: u32,
    /// The algorithm the region header names.
    pub alg: RegionAlg,
    /// The key-encryption key the footer's envelope names.
    pub key_id: &'a KeyId,
}

impl RegionAad<'_> {
    /// The bytes handed to the AEAD as associated data.
    ///
    /// ⚠️ **Length-prefixed, not delimited**, for the reason
    /// `FakeKeyProvider::tag` in `oqueue-core` already carries: a delimiter
    /// lets two different identities encode to bytes one is a prefix of, and
    /// an AAD collision is a region that opens under the wrong label. Every
    /// field here is either length-prefixed or fixed-width, so the encoding is
    /// injective.
    ///
    /// ⚠️ **The domain prefix is part of it.** It keeps this associated data
    /// from ever being confused with another structure's, should a later
    /// milestone seal something that is not a region under the same key.
    ///
    /// ⚠️ **Public so a test can pin the bytes**, and that is the reason
    /// rather than a caller needing them: every other test in this crate
    /// round-trips `seal` through `open`, so a refactor that moved this
    /// encoding — little-endian fields, a dropped domain prefix, two fields
    /// swapped — would pass all of them while making every previously sealed
    /// region unopenable forever. `region::the_associated_data_encoding_is_pinned`
    /// is the known-answer test that fails instead. ⚠️ **These bytes are
    /// format, not implementation**: `M13`'s `aws-lc-rs` build must produce
    /// exactly them.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let topic = self.topic.as_str().as_bytes();
        let mut out = Vec::with_capacity(AAD_DOMAIN.len() + topic.len() + 18);
        out.extend_from_slice(AAD_DOMAIN);
        // A `u64` length prefix, so the field is fixed-width whatever the
        // name's length is — `TopicId` is already bounded far below this.
        out.extend_from_slice(&(topic.len() as u64).to_be_bytes());
        out.extend_from_slice(topic);
        out.extend_from_slice(&self.partition.get().to_be_bytes());
        out.extend_from_slice(&self.region_index.to_be_bytes());
        out.push(self.alg.code());
        // ⚠️ **Appended, and length-prefixed like the topic name**, so the
        // encoding stays injective: without the prefix a key id could absorb
        // the boundary and two identities would encode alike.
        let key_id = self.key_id.as_str().as_bytes();
        out.extend_from_slice(&(key_id.len() as u64).to_be_bytes());
        out.extend_from_slice(key_id);
        out
    }
}

/// The domain separator every region's associated data starts with.
const AAD_DOMAIN: &[u8] = b"oqueue:region:v1";

/// Seals a region's bytes under `dek`, returning ciphertext ‖ tag.
///
/// ⚠️ **Takes the [`Nonce`] by value and consumes it.** `nonce.rs` says why in
/// full: the nonce type is linear so that a flush which seals, fails to
/// upload, and rebuilds a region cannot reuse the nonce it already spent. Two
/// plaintexts under one key and one nonce is the catastrophic failure of this
/// milestone, and this signature is where the type system gets to prevent it.
///
/// # Errors
///
/// [`Error::RegionNotEncrypted`] if `aad.alg` is [`RegionAlg::None`] — sealing
/// under "stored as written" is a caller that has not decided whether this
/// region is encrypted, and writing the plaintext back would answer it the
/// dangerous way.
///
/// [`Error::RegionSealFailed`] only for a region larger than AES-GCM's own
/// per-message bound — unreachable under `ADR-0050` point 3's rotation bound,
/// and an error rather than a partial seal.
#[expect(
    clippy::needless_pass_by_value,
    reason = "the whole point of the signature: `Nonce` is linear (see \
              `oqueue-core::nonce`), so sealing *consumes* the nonce it \
              spends. Taking `&Nonce` — which is what the lint asks for, \
              since the body only reads the bytes — would leave the caller \
              holding a nonce that still looks usable after it has been \
              used, and two plaintexts under one key and one nonce is the \
              catastrophic failure this milestone is built to make \
              unwritable"
)]
pub fn seal(dek: &Dek, nonce: Nonce, aad: RegionAad<'_>, plaintext: &[u8]) -> Result<Vec<u8>> {
    match aad.alg {
        RegionAlg::None => Err(Error::RegionNotEncrypted),
        RegionAlg::Aes256Gcm => {
            // ⚠️ The nonce is spent here: `nonce` is owned by this call and
            // dies with it, so nothing downstream holds a value that still
            // looks usable.
            let bytes = *nonce.as_bytes();
            Aes256Gcm::new(dek.expose().into())
                .encrypt(
                    &bytes.into(),
                    Payload {
                        msg: plaintext,
                        aad: &aad.encode(),
                    },
                )
                // Only reachable for a region longer than GCM's own
                // per-message bound, which `ADR-0050` point 3's 64 GiB
                // rotation bound already sits under — an error, never a
                // partial seal.
                .map_err(|_| Error::RegionSealFailed)
        }
    }
}

/// Opens a region sealed by [`seal`], under the algorithm the header named.
///
/// ⚠️ **`aad.alg` is the value read out of the region header**, and it is both
/// what selects the implementation and part of what the tag covers. So an
/// attacker who edits the header's algorithm byte does not get the region
/// decoded under a different algorithm; they get [`Error::RegionOpenFailed`],
/// because the associated data no longer matches.
///
/// ⚠️ Takes a [`ParsedNonce`] — the decode-only twin of [`Nonce`] — so twelve
/// bytes that came out of an object can never be fed to [`seal`].
///
/// # Errors
///
/// [`Error::RegionNotEncrypted`] if `aad.alg` is [`RegionAlg::None`]: bytes
/// stored as written are not openable, and handing them back as "plaintext"
/// would let a reader that asked for decryption receive whatever the object
/// held.
///
/// [`Error::RegionOpenFailed`] if the region does not authenticate — a wrong
/// key, a wrong nonce, altered ciphertext, an altered tag, a different
/// identity, or bytes too short to hold a tag. ⚠️ **One error for all of
/// them**, deliberately; see the variant's own documentation.
pub fn open(dek: &Dek, nonce: ParsedNonce, aad: RegionAad<'_>, sealed: &[u8]) -> Result<Vec<u8>> {
    match aad.alg {
        RegionAlg::None => Err(Error::RegionNotEncrypted),
        RegionAlg::Aes256Gcm => Aes256Gcm::new(dek.expose().into())
            .decrypt(
                nonce.as_bytes().into(),
                Payload {
                    msg: sealed,
                    aad: &aad.encode(),
                },
            )
            // ⚠️ Bytes too short to hold a tag land here too, and that is
            // deliberate: a truncated region is exactly as unauthentic as a
            // forged one, and a caller has the same thing to do about it.
            .map_err(|_| Error::RegionOpenFailed),
    }
}
