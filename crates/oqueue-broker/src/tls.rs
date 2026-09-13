//! TLS termination — `M9.5`, `ADR-0012`'s `ring` default build.
//!
//! ⚠️ **The capability, not the deployment.** This module builds a
//! [`rustls::ServerConfig`] from certificate/key material and exposes
//! [`tokio_rustls::TlsAcceptor`] to perform the handshake — nothing here
//! decides whether `bin/oqueue serve`'s listener runs TLS-only, both
//! protocols on separate ports, or something else. `serve_connection`
//! (`connection.rs`) is already generic over `S: AsyncRead + AsyncWrite`,
//! so a [`tokio_rustls::server::TlsStream`] is a drop-in `S` the moment a
//! composer has one. ⚠️ **`M4.18` built that composer**: `bin/oqueue serve`
//! terminates TLS on the connection's own task when `OQUEUE_TLS_CERT` and
//! `OQUEUE_TLS_KEY` name a pair. The listener *model* — TLS-only, or both
//! protocols on separate ports — is still one deployment's answer and not
//! this module's.
//!
//! ⚠️ **`install_ring_provider` is this crate's own copy, not a call into
//! `oqueue-store`'s** (`crates/oqueue-store/src/tls.rs`) — that function is
//! `pub(crate)`, deliberately not reachable outside its own crate (its own
//! doc: "no reason to be reachable outside this crate at all"). The
//! underlying `rustls::crypto::ring::default_provider().install_default()`
//! call is safe to make from more than one crate in the same process by
//! design — it is `Once`-guarded here exactly the way `oqueue-store`'s own
//! copy is, and a second install from a different crate is the ordinary,
//! expected case the `Once` exists for, not a race either copy needs to
//! coordinate with the other about.

use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::{Arc, Once};

pub use tokio_rustls::TlsAcceptor;

static INSTALL: Once = Once::new();

/// Installs `ring`'s [`rustls::crypto::CryptoProvider`] as the process
/// default, if none has been installed yet. Call this before building any
/// [`ServerConfig`](rustls::ServerConfig) — `rustls` needs one before it
/// can construct a TLS session, and `default-features = false` on this
/// workspace's `rustls` pin (`ADR-0012`) means none is chosen at compile
/// time.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn install_ring_provider() {
    INSTALL.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Why a certificate or key could not be turned into a [`ServerConfig`](rustls::ServerConfig).
#[derive(Debug, thiserror::Error)]
pub enum TlsConfigError {
    /// The certificate chain's PEM could not be parsed.
    #[error("certificate PEM is malformed: {0}")]
    MalformedCertificate(#[source] rustls_pki_types::pem::Error),
    /// The certificate PEM parsed but named no certificate at all.
    #[error("certificate PEM contains no certificate")]
    NoCertificate,
    /// The private key's PEM could not be parsed.
    #[error("private key PEM is malformed: {0}")]
    MalformedPrivateKey(#[source] rustls_pki_types::pem::Error),
    /// The certificate and key were both well-formed, but `rustls` refused
    /// the pairing (e.g. the key does not match the certificate).
    #[error("rustls rejected the certificate/key pair: {0}")]
    Rejected(#[source] rustls::Error),
}

/// Builds a TLS server configuration from PEM-encoded certificate chain and
/// private key bytes.
///
/// ⚠️ **No client authentication** — this broker authenticates over
/// `SASL/PLAIN` (`ADR-0032`), inside the TLS session, not via a client
/// certificate. `rustls::ServerConfig::builder()`'s default
/// (`with_no_client_auth`) is the correct choice here, not an omission to
/// revisit.
///
/// # Errors
/// [`TlsConfigError`] if either PEM is malformed, empty, or `rustls`
/// refuses the pairing.
pub fn server_config(
    cert_chain_pem: &[u8],
    private_key_pem: &[u8],
) -> Result<Arc<rustls::ServerConfig>, TlsConfigError> {
    install_ring_provider();

    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_chain_pem)
        .collect::<Result<_, _>>()
        .map_err(TlsConfigError::MalformedCertificate)?;
    if certs.is_empty() {
        return Err(TlsConfigError::NoCertificate);
    }
    let key = PrivateKeyDer::from_pem_slice(private_key_pem)
        .map_err(TlsConfigError::MalformedPrivateKey)?;

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(TlsConfigError::Rejected)?;
    Ok(Arc::new(config))
}

/// A [`TlsAcceptor`] built from `server_config`'s output — the composer's
/// one call to get from PEM bytes to something that can
/// [`accept`](TlsAcceptor::accept) a stream.
///
/// # Errors
/// As [`server_config`].
pub fn acceptor(
    cert_chain_pem: &[u8],
    private_key_pem: &[u8],
) -> Result<TlsAcceptor, TlsConfigError> {
    Ok(TlsAcceptor::from(server_config(
        cert_chain_pem,
        private_key_pem,
    )?))
}

#[cfg(test)]
mod tests;
