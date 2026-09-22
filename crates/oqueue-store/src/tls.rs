//! Installs the selected `rustls` crypto provider.
//!
//! ⚠️ **Why this exists as code, not just a Cargo feature.** ADR-0012 picks
//! `ring` over `aws-lc-rs` for the default build, but neither `reqwest`'s nor
//! `rustls`'s Cargo features can express "TLS, with `ring`" without also
//! pulling in `aws-lc-rs` somewhere in the graph — see the ADR's "mechanism"
//! section. The workaround is `rustls-no-provider`: TLS wired up with no
//! provider selected at compile time, which then needs exactly one runtime
//! call, made before any TLS connection, to pick one. This module is that
//! call.

use std::sync::Once;

static INSTALL: Once = Once::new();

/// Installs the build's [`rustls::crypto::CryptoProvider`] as the process
/// default: AWS-LC in FIPS builds, Ring otherwise.
///
/// Call this before constructing any client that will make an HTTPS
/// connection (every `oqueue-store` backend's constructor does). Idempotent
/// and safe to call from more than one backend's constructor in the same
/// process — the `Once` makes every call after the first a no-op.
///
/// `rustls::crypto::ring::default_provider().install_default()` fails only
/// when a provider was already installed, which just means some other call
/// (this one, racing itself, or another crate in the same process) already
/// gave TLS what this function exists to guarantee. Either way a provider is
/// installed by the time this returns, so the failure carries nothing this
/// crate needs to report.
// ⚠️ `pub(crate)`, not `pub`: `mod tls` is private, so `clippy::redundant_pub_crate`
// (correctly, in general) suggests plain `pub` is equivalent — but this
// module has no reason to be reachable outside this crate at all, and typing
// `pub` here would be a standing invitation for that to happen by accident
// later. Same trade `rust-style.md` calls out for `unreachable_pub`-shaped
// conflicts elsewhere in this crate.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn install_ring_provider() {
    INSTALL.call_once(|| {
        #[cfg(feature = "fips")]
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        #[cfg(not(feature = "fips"))]
        let _ = rustls::crypto::ring::default_provider().install_default();

        let Some(provider) = rustls::crypto::CryptoProvider::get_default() else {
            panic!("TLS provider installation left no process default");
        };
        assert_eq!(
            provider.fips(),
            cfg!(feature = "fips"),
            "TLS provider does not match the artifact's FIPS mode"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::install_ring_provider;

    // The workspace denies `expect_used`/`unwrap_used` outside test files;
    // this file's only non-test code never unwraps anything, so the file-wide
    // allow some other test files need does not apply here.

    #[test]
    fn installing_twice_does_not_panic() {
        install_ring_provider();
        install_ring_provider();
    }

    #[test]
    fn a_provider_is_installed_afterward() {
        install_ring_provider();
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "install_ring_provider must leave a default provider installed"
        );
    }
}
