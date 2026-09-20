//! AWS and GCP KMS adapters over the common [`oqueue_core::KeyProvider`] seam.
//!
//! The API traits here are deliberately smaller than either vendor SDK: M8
//! proves the adapter shape against in-process simulations, while M15 owns the
//! real-cloud round trips. Both APIs expose only encrypt/decrypt operations;
//! neither can accidentally grow a `generate_data_key` method that GCP cannot
//! implement.

use oqueue_core::{BoxFuture, KeyId, KeyProvider, Redacted, Result, WrappedKey};

/// The AWS KMS `Encrypt`/`Decrypt` operations needed by the provider adapter.
///
/// A composition root supplies the real SDK implementation later. The trait
/// keeps this crate independent of an SDK and lets M8 test the request/response
/// mapping against an in-process API simulation.
pub trait AwsKmsApi: Send + Sync + core::fmt::Debug {
    /// Encrypts plaintext under the named AWS KMS key.
    fn encrypt<'a>(
        &'a self,
        key_id: &'a str,
        plaintext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>>;

    /// Decrypts an AWS KMS ciphertext under the named key.
    fn decrypt<'a>(
        &'a self,
        key_id: &'a str,
        ciphertext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>>;
}

/// An AWS KMS adapter implementing the common key-provider seam.
#[derive(Debug)]
pub struct AwsKmsProvider<A> {
    api: A,
}

impl<A> AwsKmsProvider<A> {
    /// Builds an adapter over an AWS KMS API implementation.
    #[must_use]
    pub const fn new(api: A) -> Self {
        Self { api }
    }

    /// The API implementation behind this adapter.
    #[must_use]
    pub const fn api(&self) -> &A {
        &self.api
    }
}

impl<A: AwsKmsApi> KeyProvider for AwsKmsProvider<A> {
    fn wrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        plaintext: &'a Redacted<Vec<u8>>,
    ) -> BoxFuture<'a, Result<WrappedKey>> {
        Box::pin(async move {
            self.api
                .encrypt(key_id.as_str(), plaintext.expose())
                .await
                .map(|ciphertext| WrappedKey::new(Redacted::new(ciphertext)))
        })
    }

    fn unwrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        wrapped: &'a WrappedKey,
    ) -> BoxFuture<'a, Result<Redacted<Vec<u8>>>> {
        Box::pin(async move {
            self.api
                .decrypt(key_id.as_str(), wrapped.as_redacted().expose())
                .await
                .map(Redacted::new)
        })
    }
}

/// The GCP Cloud KMS `encrypt`/`decrypt` operations needed by the provider
/// adapter.
///
/// GCP calls the operations lowercase in its API surface, but the seam stays
/// the same as AWS: a provider wraps and unwraps an already-generated DEK.
pub trait GcpCloudKmsApi: Send + Sync + core::fmt::Debug {
    /// Encrypts plaintext under the named GCP Cloud KMS key.
    fn encrypt<'a>(
        &'a self,
        key_id: &'a str,
        plaintext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>>;

    /// Decrypts a GCP Cloud KMS ciphertext under the named key.
    fn decrypt<'a>(
        &'a self,
        key_id: &'a str,
        ciphertext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>>;
}

/// A GCP Cloud KMS adapter implementing the common key-provider seam.
#[derive(Debug)]
pub struct GcpKmsProvider<G> {
    api: G,
}

impl<G> GcpKmsProvider<G> {
    /// Builds an adapter over a GCP Cloud KMS API implementation.
    #[must_use]
    pub const fn new(api: G) -> Self {
        Self { api }
    }

    /// The API implementation behind this adapter.
    #[must_use]
    pub const fn api(&self) -> &G {
        &self.api
    }
}

impl<G: GcpCloudKmsApi> KeyProvider for GcpKmsProvider<G> {
    fn wrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        plaintext: &'a Redacted<Vec<u8>>,
    ) -> BoxFuture<'a, Result<WrappedKey>> {
        Box::pin(async move {
            self.api
                .encrypt(key_id.as_str(), plaintext.expose())
                .await
                .map(|ciphertext| WrappedKey::new(Redacted::new(ciphertext)))
        })
    }

    fn unwrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        wrapped: &'a WrappedKey,
    ) -> BoxFuture<'a, Result<Redacted<Vec<u8>>>> {
        Box::pin(async move {
            self.api
                .decrypt(key_id.as_str(), wrapped.as_redacted().expose())
                .await
                .map(Redacted::new)
        })
    }
}
