//! AEAD, envelope encryption, the DEK cache, and nonce construction.
//!
//! Because BYOK is a per-topic opt-in and server-side encryption cannot express
//! it: one object carries many tenants' data and can only carry one SSE-KMS
//! key. So encryption is broker-side, and this is where it lives.
//!
//! ⚠️ **Still filling in.** `M0.11` added the no-op provider so the
//! unencrypted path is an explicit, testable configuration rather than an
//! absence; `M8.3` added [`region`], which seals and opens **one** region;
//! `M8.5` added [`dek_cache`] and [`unwrap_cache`], which are what keep KMS
//! calls a function of rotation rather than of produce volume (`NFR-33`), and
//! [`entropy`], the seam a fresh DEK's 32 bytes come from. The KMS providers
//! themselves are the rest of `M8`'s.
//!
//! # ⚠️ The algorithm is a format, not a library
//!
//! [`region::seal`] and [`region::open`] are AES-256-GCM over `RustCrypto`'s
//! pure-Rust `aes-gcm`, chosen so the default build still needs only cargo and
//! a C compiler (NFR-42, `ADR-0012`). `M13`'s FIPS build swaps that
//! implementation for `aws-lc-rs`'s validated one — and **swaps nothing about
//! the format**: the same `alg` code, the same nonce, the same associated
//! data, the same tag. That is the whole reason the region header names an
//! algorithm at all (`ADR-0050` point 7), and why a FIPS and a non-FIPS broker
//! can read each other's objects.
#![forbid(unsafe_code)]

pub mod dek_cache;
pub mod entropy;
pub mod region;
pub mod unwrap_cache;

pub use dek_cache::{DEK_MAX_AGE_MS, DEK_MAX_SEALED_BYTES, DekCache};
pub use entropy::{Entropy, FakeEntropy, OsEntropy, mint_dek};
pub use region::{RegionAad, TAG_BYTES, open, seal};
pub use unwrap_cache::{UNWRAPPED_DEK_CACHE_ENTRIES, UNWRAPPED_DEK_TTL_MS, UnwrappedDekCache};

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
