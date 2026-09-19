//! The key-management seam: wrap and unwrap, and nothing else.

use crate::{BoxFuture, Redacted, Result};
use subtle::ConstantTimeEq as _;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Names a key in whatever KMS holds it.
///
/// # Invariant
///
/// **Non-empty, and that is the whole invariant, deliberately.** An AWS KMS ARN
/// and a GCP resource path are both just strings here, because `oqueue-core`
/// names neither service (NFR-51), and their formats differ.
///
/// ⚠️ **No character set is imposed, so `KeyId::new("a\0")` is accepted today.**
/// A key id is interpolated into an operator-facing log line, so a control
/// character in one is a real if minor problem. Validating per provider belongs
/// to `M8`, where the provider is known; a guess here would be wrong for one
/// cloud or both.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(String);

impl KeyId {
    /// Builds a key id.
    ///
    /// # Errors
    ///
    /// [`crate::Error::EmptyKeyId`] if `id` is empty.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(crate::Error::EmptyKeyId);
        }
        Ok(Self(id))
    }

    /// The key id.
    ///
    /// ⚠️ A key **identifier**, never key material — safe to log, and the thing
    /// to log instead of anything that is not.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for KeyId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A data encryption key that has been encrypted by a key-encryption key.
///
/// Safe to store beside the data it protects; useless without the KMS.
///
/// ⚠️ Holds its bytes in [`Redacted`], so the derived `Debug` prints
/// `WrappedKey(Redacted(<redacted>))` and there is no formatting path to the
/// ciphertext. FR-44.
///
/// ⚠️ **Deliberately not `PartialEq`.** ADR-0006 guarantee 6 says a real KMS may
/// return a different ciphertext for the same DEK on every call, so comparing
/// two wrapped keys answers nothing. Deriving it would make a cache test green
/// here and wrong against AWS and GCP.
#[derive(Debug, Clone)]
pub struct WrappedKey(Redacted<Vec<u8>>);

impl WrappedKey {
    /// Wraps already-encrypted bytes.
    ///
    /// ⚠️ **This type asserts nothing about what it holds.** It cannot: the
    /// ciphertext is opaque and only the KMS can tell. So `WrappedKey::new` over
    /// a *plaintext* DEK compiles, and is exactly the identity construction
    /// ADR-0006 rejects at the provider level — one safe line away, on the
    /// branch that ADR's "Hard" consequence forces callers to write. Every call
    /// site outside a `KeyProvider` implementation is a review question.
    #[must_use]
    pub const fn new(bytes: Redacted<Vec<u8>>) -> Self {
        Self(bytes)
    }

    /// The encrypted bytes, still redacted.
    #[must_use]
    pub const fn as_redacted(&self) -> &Redacted<Vec<u8>> {
        &self.0
    }
}

/// The length of a [`Dek`], in bytes: AES-256 takes a 256-bit key.
///
/// ⚠️ **A property of the algorithm, not a tunable.** ADR-0050 fixes the
/// envelope on AES-GCM with a 256-bit DEK; a shorter key is a different
/// algorithm, not a configuration of this one.
pub const DEK_BYTES: usize = 32;

/// A plaintext data encryption key: 32 bytes of AES-256 key material.
///
/// This is the live key `KeyProvider::unwrap` hands back and the thing
/// ADR-0006 guarantee 4 calls "the caller's to zeroize". It is that caller's
/// type.
///
/// # What it guarantees
///
/// ⚠️ **Zeroized on drop** (`security.md` rule 8). `Drop` wipes the array
/// through [`Zeroize`], and the type implements [`ZeroizeOnDrop`] to say so in
/// the type system. ⚠️ What that does *not* guarantee is that no other copy
/// survives: a `Dek` built from a slice leaves the caller's slice alone, and a
/// value moved in memory may leave the source bytes behind — only the bytes
/// this value owns at the moment it drops are wiped. `security.md` rule 9 (key
/// material never lands on disk) is a separate obligation this type cannot
/// discharge.
///
/// ⚠️ **No `Clone`, deliberately.** Every clone is another plaintext copy with
/// its own lifetime and its own chance to outlive its use, and nothing in the
/// envelope scheme needs one: a DEK is unwrapped into one owner, used, and
/// dropped. Code that thinks it needs a second copy wants a second
/// `unwrap`, or a borrow.
///
/// ⚠️ **No derived `PartialEq`.** A bytewise compare of key material is a
/// timing oracle. [`Dek::ct_eq`] is the only equality, and it is constant-time.
///
/// ⚠️ **`Debug` prints a fixed string**, as [`Redacted`] does, and takes no
/// route to the bytes — FR-44, `security.md` rules 6-7.
pub struct Dek([u8; DEK_BYTES]);

impl Dek {
    /// Takes ownership of 32 bytes of key material.
    #[must_use]
    pub const fn new(bytes: [u8; DEK_BYTES]) -> Self {
        Self(bytes)
    }

    /// Copies key material out of a slice.
    ///
    /// # Errors
    ///
    /// [`crate::Error::DekLength`] if `bytes` is not [`DEK_BYTES`] long.
    /// ⚠️ The error carries the *length*, never the bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let array: [u8; DEK_BYTES] =
            bytes
                .try_into()
                .map_err(
                    |_: core::array::TryFromSliceError| crate::Error::DekLength {
                        got: bytes.len(),
                    },
                )?;
        Ok(Self(array))
    }

    /// The key bytes, for the AEAD and nothing else.
    ///
    /// ⚠️ Named as [`Redacted::expose`] is, and for the same reason: every call
    /// is a place where key material leaves its wrapper, and that is a review
    /// question. The returned borrow must not be copied into anything that
    /// outlives the `Dek`, or rule 8's guarantee is defeated by the copy rather
    /// than by this type.
    #[must_use]
    pub const fn expose(&self) -> &[u8; DEK_BYTES] {
        &self.0
    }

    /// Whether two keys are the same, in time independent of *where* they
    /// differ.
    ///
    /// ⚠️ Constant-time in the contents, via [`subtle`]. The lengths are equal
    /// by construction, so there is no length side channel either.
    #[must_use]
    pub fn ct_eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

/// ⚠️ Prints a fixed string; no route to the bytes at any format specifier,
/// exactly as [`Redacted`]'s own `Debug` does.
impl core::fmt::Debug for Dek {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Dek(<redacted>)")
    }
}

impl Zeroize for Dek {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for Dek {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// ⚠️ A marker, not an implementation: [`Drop`] above is what does the wiping.
/// It exists so a caller can *require* the property in a bound.
impl ZeroizeOnDrop for Dek {}

/// Wraps and unwraps data encryption keys.
///
/// ⚠️ **Two methods, and deliberately no `generate_data_key`.** AWS KMS offers
/// one; GCP Cloud KMS has no equivalent (doc 22 §4), so a seam with it would be
/// a seam only one cloud can implement. The DEK is generated by
/// `oqueue-crypto` and wrapped through here.
///
/// ⚠️ **Nothing outside this seam touches key material** (NFR-51), and neither
/// the plaintext DEK nor the wrapped one can reach a formatted string: both
/// travel in [`Redacted`].
///
/// # What an implementor must guarantee
///
/// See ADR-0006: unwrap(wrap(k)) is k under the same [`KeyId`]; a `WrappedKey`
/// produced under one key id does not unwrap under another; and no error, log
/// or panic message it produces contains key material.
///
/// ADR-0006 is `docs/internal/product/decisions/0006-key-provider-seam.md` in
/// this repository.
pub trait KeyProvider: Send + Sync + core::fmt::Debug {
    /// Encrypts a data encryption key under the key-encryption key named by
    /// `key_id`.
    ///
    /// # Errors
    ///
    /// Implementation-defined. [`crate::Error::EncryptionDisabled`] from a
    /// provider configured for the unencrypted path.
    fn wrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        plaintext: &'a Redacted<Vec<u8>>,
    ) -> BoxFuture<'a, Result<WrappedKey>>;

    /// Decrypts a data encryption key.
    ///
    /// # Errors
    ///
    /// [`crate::Error::SecretRejected`] if `wrapped` was not produced under
    /// `key_id`.
    fn unwrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        wrapped: &'a WrappedKey,
    ) -> BoxFuture<'a, Result<Redacted<Vec<u8>>>>;
}

/// A [`KeyProvider`] that "encrypts" by tagging, for tests.
///
/// ⚠️ **A fake, beside its trait** — `contracts.md` rule 9. It performs **no
/// cryptography whatsoever**: `wrap` prefixes the plaintext with the key id and
/// `unwrap` checks that prefix. That is enough to catch the mistake worth
/// catching in a unit test — unwrapping under the wrong key id — and is
/// obviously not enough for anything else.
///
/// ⚠️ **Never construct this outside a test.** It stores the DEK in plaintext.
#[derive(Debug, Default)]
pub struct FakeKeyProvider;

impl FakeKeyProvider {
    /// A fake provider.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// ⚠️ **Length-prefixed, not delimited.** A `key_id || 0x00` tag aliases:
    /// a key wrapped under `"a\0"` unwraps under `"a"`, because the latter's
    /// tag is a prefix of the former's — a wrong-key unwrap that *succeeds*
    /// with a corrupted DEK, breaking the one guarantee this fake exists to
    /// model. Found in review.
    fn tag(key_id: &KeyId) -> Vec<u8> {
        let id = key_id.as_str().as_bytes();
        let mut tag = (id.len() as u64).to_be_bytes().to_vec();
        tag.extend_from_slice(id);
        tag
    }
}

impl KeyProvider for FakeKeyProvider {
    fn wrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        plaintext: &'a Redacted<Vec<u8>>,
    ) -> BoxFuture<'a, Result<WrappedKey>> {
        Box::pin(async move {
            let mut bytes = Self::tag(key_id);
            bytes.extend_from_slice(plaintext.expose());
            Ok(WrappedKey::new(Redacted::new(bytes)))
        })
    }

    fn unwrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        wrapped: &'a WrappedKey,
    ) -> BoxFuture<'a, Result<Redacted<Vec<u8>>>> {
        Box::pin(async move {
            let tag = Self::tag(key_id);
            let bytes = wrapped.as_redacted().expose();
            bytes.strip_prefix(tag.as_slice()).map_or_else(
                || {
                    Err(crate::Error::SecretRejected {
                        context: "unwrapping a data encryption key",
                        key_id: key_id.clone(),
                    })
                },
                |plaintext| Ok(Redacted::new(plaintext.to_vec())),
            )
        })
    }
}
