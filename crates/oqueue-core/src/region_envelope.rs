//! What a sealed region's footer carries beside its byte range:
//! `{key_id, wrapped_dek, nonce}`.
//!
//! `ADR-0050` point 1 names four things a sealed region carries — `{key_id,
//! wrapped_dek, nonce, alg}`. The fourth already lives in the region header as
//! [`RegionAlg`](crate::RegionAlg), written since `M3`; this type is the other
//! three, and it exists **only** for a region whose algorithm is not
//! [`RegionAlg::None`](crate::RegionAlg::None).
//!
//! ⚠️ **Its own module because it is durable format with its own bounds.** Every
//! field here is written into an object and read back out of bytes an object
//! store returned, so each one needs a width the footer can express and a
//! refusal for anything wider — `security.md` rule 3. Keeping the bounds beside
//! the type is what stops the reader and the writer disagreeing about them.

use crate::{Error, KeyId, ParsedNonce, RegionAlg, Result, WrappedKey};

/// The longest key id a footer can carry.
///
/// ⚠️ A bound the *format* imposes — what the footer's `u16` length field can
/// express — not a policy about key names, exactly as
/// `MAX_TOPIC_NAME_LEN` is. An AWS KMS ARN and a GCP resource path are both
/// orders of magnitude below it.
pub const MAX_KEY_ID_LEN: usize = u16::MAX as usize;

/// The longest wrapped DEK a footer will carry, in bytes.
///
/// ⚠️ **A real bound, and it weakens by rising.** The length field is a `u32`,
/// so without this a footer could claim four gigabytes for one region's key
/// blob and a parser that trusted the claim would allocate it. Sized well above
/// what either provider produces — an AWS KMS `CiphertextBlob` for a 32-byte
/// plaintext is a few hundred bytes and GCP's is comparable — so the refusal is
/// unreachable for a real wrapped key and immediate for a fabricated length.
pub const MAX_WRAPPED_DEK_LEN: usize = 8192;

/// The envelope a sealed region carries in the footer.
///
/// # ⚠️ What a tamperer can and cannot do to these bytes
///
/// The `key_id` and the `nonce` are both covered: the key id is in the region's
/// associated data (`oqueue-crypto`'s `RegionAad`) and the nonce is an input to
/// AES-GCM itself, so editing either makes the region fail to open rather than
/// open differently. The **wrapped DEK's bytes are not covered**, and that is
/// forced rather than chosen — see [`RegionEnvelope::wrapped_dek`].
#[derive(Debug, Clone)]
pub struct RegionEnvelope {
    key_id: KeyId,
    wrapped_dek: WrappedKey,
    nonce: ParsedNonce,
}

impl RegionEnvelope {
    /// Builds the envelope for one sealed region.
    ///
    /// ⚠️ **Takes a [`ParsedNonce`], on both paths, and that is deliberate.**
    /// The envelope is the footer's *record* of the nonce — twelve public bytes
    /// — rather than the linear permission to seal, which is
    /// [`Nonce`](crate::Nonce) and which `oqueue-crypto::seal` consumes. A
    /// writer derives one from the other and then spends the `Nonce`; there is
    /// still no path from these bytes back to a `Nonce`, so an envelope read
    /// out of an object can never authorise a seal.
    ///
    /// # Errors
    ///
    /// [`Error::RegionEnvelopeLength`] if the key id is longer than
    /// [`MAX_KEY_ID_LEN`], or the wrapped DEK is empty or longer than
    /// [`MAX_WRAPPED_DEK_LEN`]. ⚠️ Checked here, where the value is first
    /// named, rather than at encode time when a payload is already built — and
    /// the same refusal is what the parser applies to a claimed length, so a
    /// footer cannot describe an envelope this type would not accept.
    pub fn new(key_id: KeyId, wrapped_dek: WrappedKey, nonce: ParsedNonce) -> Result<Self> {
        let id_len = key_id.as_str().len();
        if id_len > MAX_KEY_ID_LEN {
            return Err(Error::RegionEnvelopeLength {
                field: "key id",
                got: id_len,
            });
        }
        let wrapped_len = wrapped_dek.as_redacted().expose().len();
        if wrapped_len == 0 || wrapped_len > MAX_WRAPPED_DEK_LEN {
            return Err(Error::RegionEnvelopeLength {
                field: "wrapped data encryption key",
                got: wrapped_len,
            });
        }
        Ok(Self {
            key_id,
            wrapped_dek,
            nonce,
        })
    }

    /// Which key-encryption key wrapped this region's DEK.
    ///
    /// ⚠️ A key **identifier**, never key material — and it is bound into the
    /// region's associated data, so an object whose footer names a different
    /// key does not open.
    #[must_use]
    pub const fn key_id(&self) -> &KeyId {
        &self.key_id
    }

    /// The region's data encryption key, encrypted under [`Self::key_id`].
    ///
    /// ⚠️ **These bytes are the one part of the envelope outside the tag**, and
    /// `ADR-0050` point 4 is why: a DEK is re-wrapped lazily under the current
    /// KEK version, which changes the wrapped bytes for an unchanged key, so
    /// binding them would make every re-wrap a decrypt-and-reseal of the data.
    /// ⚠️ What that concedes is bounded and worth stating: substituting these
    /// bytes yields either an unwrap failure at the KMS or a different DEK, and
    /// a different DEK fails the tag. So the exposure is a denial of service on
    /// one region, never a region opening as something it is not.
    #[must_use]
    pub const fn wrapped_dek(&self) -> &WrappedKey {
        &self.wrapped_dek
    }

    /// The twelve bytes the region was sealed under.
    #[must_use]
    pub const fn nonce(&self) -> ParsedNonce {
        self.nonce
    }
}

/// ⚠️ **"The same footer bytes", not "the same key".**
///
/// [`WrappedKey`] deliberately has no `PartialEq`, because `ADR-0006`
/// guarantee 6 lets a KMS return different ciphertext for one DEK on every
/// call — so comparing two wrapped keys answers nothing about the keys. The
/// question *this* impl answers is the round-trip one: are these the bytes the
/// footer wrote? That is a question about an encoding, and it is the only
/// reason [`Region`](crate::Region) is comparable at all. ⚠️ Still
/// constant-time in the wrapped bytes, through [`Redacted`](crate::Redacted)'s
/// own equality, so nothing here becomes a timing oracle if a caller reaches
/// for it with a secret in hand.
impl PartialEq for RegionEnvelope {
    fn eq(&self, other: &Self) -> bool {
        self.key_id == other.key_id
            && self.nonce == other.nonce
            && self.wrapped_dek.as_redacted() == other.wrapped_dek.as_redacted()
    }
}

impl Eq for RegionEnvelope {}

/// One sealed region's bytes and the envelope that opens them.
///
/// ⚠️ **Grouped so [`BundleBuilder::push_sealed`](crate::BundleBuilder::push_sealed) stays at `clippy.toml`'s
/// five-argument threshold**, the same reason [`PushedRecords`](crate::PushedRecords) groups its
/// two — and the grouping is the natural one: these three are exactly what
/// distinguishes a sealed push from an ordinary one.
#[derive(Debug)]
pub struct SealedRegion<'a> {
    /// The region's ciphertext ‖ tag, as `oqueue-crypto::seal` returned it.
    ///
    /// ⚠️ **Already sealed.** This type performs no cryptography and names no
    /// algorithm implementation — `oqueue-core` is sans-I/O and sans-cipher,
    /// and FR-43's agnosticism lives in the fact that the footer carries an
    /// algorithm code rather than in anything here knowing what it means.
    pub bytes: &'a [u8],
    /// The algorithm the region was sealed under, which the footer records and
    /// a reader dispatches on.
    pub alg: RegionAlg,
    /// The key id, wrapped DEK and nonce the footer will carry.
    pub envelope: RegionEnvelope,
}
