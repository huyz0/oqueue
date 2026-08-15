//! AEAD, envelope encryption, the DEK cache, and nonce construction.
//!
//! Because BYOK is a per-topic opt-in and server-side encryption cannot express
//! it: one object carries many tenants' data and can only carry one SSE-KMS
//! key. So encryption is broker-side, and this is where it lives.
//!
//! ⚠️ **Almost empty.** `M0.11` added the no-op provider so the unencrypted
//! path is an explicit, testable configuration rather than an absence; the
//! envelope scheme, the DEK cache and nonce construction are `M8`'s.
#![forbid(unsafe_code)]

use oqueue_core::{BoxFuture, Error, KeyId, KeyProvider, Redacted, Result, WrappedKey};

/// The [`KeyProvider`] for a deployment with encryption turned off.
///
/// ⚠️ **Production code, not a test double** — which is why it lives here and
/// not beside the trait in `oqueue-core`. `contracts.md` rule 9 puts *fakes*
/// there; this is the real implementation of "no BYOK configured".
///
/// # Why it refuses rather than passing bytes through
///
/// An identity provider — returning the plaintext DEK as its own wrapped form —
/// would make every call site succeed and would write a **plaintext data
/// encryption key** into object storage beside the data it protects, looking
/// exactly like a successful wrap. Refusing turns a misconfiguration into an
/// error at the first call instead of a silent, durable compromise. ADR-0006.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpKeyProvider;

impl NoOpKeyProvider {
    /// A provider for a deployment that does not encrypt.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl KeyProvider for NoOpKeyProvider {
    fn wrap<'a>(
        &'a self,
        _key_id: &'a KeyId,
        _plaintext: &'a Redacted<Vec<u8>>,
    ) -> BoxFuture<'a, Result<WrappedKey>> {
        Box::pin(async { Err(Error::EncryptionDisabled) })
    }

    fn unwrap<'a>(
        &'a self,
        _key_id: &'a KeyId,
        _wrapped: &'a WrappedKey,
    ) -> BoxFuture<'a, Result<Redacted<Vec<u8>>>> {
        Box::pin(async { Err(Error::EncryptionDisabled) })
    }
}
