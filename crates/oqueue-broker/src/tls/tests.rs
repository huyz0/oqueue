#![allow(clippy::expect_used)]

use super::{TlsConfigError, acceptor, install_ring_provider, server_config};

/// A self-signed test certificate/key, `CN=oqueue-test`, `SAN: DNS:oqueue-test`.
///
/// ⚠️ **Test-only, generated once with `openssl req -x509 -newkey rsa:2048
/// ... -addext 'subjectAltName=DNS:oqueue-test' -addext
/// 'basicConstraints=critical,CA:FALSE' -addext
/// 'keyUsage=critical,digitalSignature,keyEncipherment' -addext
/// 'extendedKeyUsage=serverAuth'`, 10-year validity.** Never used by
/// anything outside this module. Two properties are load-bearing, not
/// decorative, and each one's omission was caught by running this test
/// suite against an earlier, simpler `openssl req -x509` cert: the `SAN`
/// (a modern `rustls`/`webpki` client refuses to verify a server
/// certificate against a bare CN with no `subjectAltName`, RFC 6125), and
/// `basicConstraints=CA:FALSE` (without it, `openssl req -x509`'s
/// self-signed output verifies as a CA certificate, and `webpki` refuses
/// to accept a CA certificate presented as a TLS server's own end-entity
/// leaf — `CaUsedAsEndEntity`).
const TEST_CERT_PEM: &[u8] = br"-----BEGIN CERTIFICATE-----
MIIDSTCCAjGgAwIBAgIUJabZOBs1YBF491KjE6CpxEJ3rzgwDQYJKoZIhvcNAQEL
BQAwFjEUMBIGA1UEAwwLb3F1ZXVlLXRlc3QwHhcNMjYwOTAyMTExNjUwWhcNMzYw
ODMwMTExNjUwWjAWMRQwEgYDVQQDDAtvcXVldWUtdGVzdDCCASIwDQYJKoZIhvcN
AQEBBQADggEPADCCAQoCggEBALmYDUsU9BU1H+pech38TE2crKX4744RL99AjZAE
RuI114sPRIgtKQleE5uyz7eDCwcMYZKQAvvWBjP5uy2eVOqbDB6p7Z/ZQCHrcSr6
ST32Wz9Gmgbr4y6e0HkxHlL2hjSQ57WibU3pyQZ/a7EQcFVVnbeVDAtYTKvDbFXc
OEr92ZuSw+lvCqk1XZyoWpYWSJQPdj+eAh6SKkQ2XEZez3UtqDQUiNlf7AnEstRN
WtIwpJyTfW7k9gnF09iyzoGSTc50hGg7WqlX+lTbjFrv1hOYN/gG1hnTW+8YROFa
crkSod+K4syMnQ/vLE8S8EgMq/4QCt8VHXLMoAp0wDuM6jECAwEAAaOBjjCBizAd
BgNVHQ4EFgQUWLYViLeMeOAXo7OXQ7IVQAV5XTIwHwYDVR0jBBgwFoAUWLYViLeM
eOAXo7OXQ7IVQAV5XTIwFgYDVR0RBA8wDYILb3F1ZXVlLXRlc3QwDAYDVR0TAQH/
BAIwADAOBgNVHQ8BAf8EBAMCBaAwEwYDVR0lBAwwCgYIKwYBBQUHAwEwDQYJKoZI
hvcNAQELBQADggEBACF+t70iWvSBgXs5RbwhIv+4uu9X7uE7BTCDZ3mcpd6DopbY
mSrGqAbDx5J4GdstOcFtrxKpUl8AIDTJ7FnP8ZyTLaPw5hJtLLWUe+EGXCduqOcq
P9ByBbYzOeheN5pxasCaP3WffkIJkyiONngxuxOCP9wd+vNBRoL9tUXbLtObN3Pr
x1UsBHBve3yaMdzIjn/E4QbloHjPL3NpwpqEuvnZPzZW2b5xaRtXfeiSwE536tnY
UhqHHOZZe5/aIJeTr/ZqYCkXNcCKNsecLD4vT9JRh/ckTGg5E/RhrrQCwHqjF6p2
f1uwMnSotmiPF0Z5XBAvMnoJhMFJrxBsYR07dwg=
-----END CERTIFICATE-----
";

const TEST_KEY_PEM: &[u8] = br"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC5mA1LFPQVNR/q
XnId/ExNnKyl+O+OES/fQI2QBEbiNdeLD0SILSkJXhObss+3gwsHDGGSkAL71gYz
+bstnlTqmwweqe2f2UAh63Eq+kk99ls/RpoG6+MuntB5MR5S9oY0kOe1om1N6ckG
f2uxEHBVVZ23lQwLWEyrw2xV3DhK/dmbksPpbwqpNV2cqFqWFkiUD3Y/ngIekipE
NlxGXs91Lag0FIjZX+wJxLLUTVrSMKSck31u5PYJxdPYss6Bkk3OdIRoO1qpV/pU
24xa79YTmDf4BtYZ01vvGEThWnK5EqHfiuLMjJ0P7yxPEvBIDKv+EArfFR1yzKAK
dMA7jOoxAgMBAAECggEADntKmD/1jr0TNTSy51uTMaAmyZmbyaRWLa+qDCGFTWvh
mnhxwsVxVQmJ8qV4d0uKnf1tlKPXk8J+v+n93MCkxByejLr6L3WifzMRpMacVfEl
7BFMfftEghQC1N4MDXGuhaYD6oSWzlROawsgwlNzzHjOgm9nHfCBJQrt5mI1Y0Z6
s0xg4j5oc3Z7elcmculXlksjsSuq64oI3ci+r0YR5dwU3mXZk0MeV1of876St7Pt
YznTFYx41Dbwb6+xfaAIMrHZFODPpoidZ9V+2Y16s3F5qQEUsBz4Z7aZCQlwL6qj
/Qc9SzBg67Wg41MMMuinGjApeROBpfAwsG4N05jrBQKBgQD5folRLcLUzbH0uSMm
8z2Obueu+IBEl6EkzXdX5iM2qn4Opa6xU2mYiOswdXVk8th4+AxsVc8Cfu8SN88r
rYfAmVvR47ExRXAg8pEm3nbgSh/m+r0S1CySkz9jON6h6OHguqf9nnDlmTQF4pfo
ecpdv7qAoKAKPLZEFIRkXVq9lQKBgQC+bvU9SWf3o1K0Bt6b325VtzwhBZfcIQ6P
54c88awwLhiHbDH0c/fSpcJHN6HIoN+YATrru9Tj+fx1FCkzGdms/1QbJYql+aAK
HvnTjkC0KScpgTZN7n/s+wdjaOhJdIy753zKwF2d2mZ3gzlclhxCNjYEu0klz87d
loKf5Ld7LQKBgQCxw6XFUGycQT8FVhAkxXTbkkvDUE3cEYmAdmENIO2AGrQcbZJt
yDfZtdyVN2uAlMMGVf5MBkurxJNEkL0sqsSpxts0Th5HM+lzoEEpx6I9prLaWVb0
HnbvrLiiUrfV9t9RxszBGO3puWHmu49u1bAJYf1ZfpjpEl7vXQsDk7x+jQKBgC9x
5ZfHWifQgSJpM70SBaNFa62ufw9RDRe9T2xXqda3JVVYF3oYCn5o3eZwbdZWfl6Y
r91bhsbl2Ygx5bHdluYLFyFMUSbY8o6S+RtELcq1FhS5JJZ1/VlFkamq0XS7nPST
z/uTwb86Up0kDH6Mx62XZA35u1e4VonOnezIRw5hAoGAdBn2wSOxjYk6z9JfuP26
op89S+ZhRsFbCtnyOJhrG94guGrFgalinykf6qmavnqepwlFzYcbS6jtmAKMqNym
/k+tM2w0VABASirVmzvns2qV8RblR5U8qfHBayZc3TVLXnQhlZyWH5rbqUWxqKMs
QSae45MjrYYeDRpj1qmCRtw=
-----END PRIVATE KEY-----
";

/// `oqueue-store/src/tls.rs`'s own precedent, mirrored: a mutant replacing
/// `install_ring_provider`'s body with `()` is caught only by checking the
/// actual side effect, not by `server_config`'s own tests, which succeed
/// either way if some *other* crate in the same test binary already
/// installed a provider first (`rustls::crypto::CryptoProvider` is a
/// process-global, and this crate's own `a_real_handshake_completes_and_
/// carries_bytes` calls it too by construction).
#[test]
fn a_provider_is_installed_afterward() {
    install_ring_provider();
    assert!(
        rustls::crypto::CryptoProvider::get_default().is_some(),
        "install_ring_provider must leave a default provider installed"
    );
}

#[test]
fn installing_twice_does_not_panic() {
    install_ring_provider();
    install_ring_provider();
}

#[test]
fn a_valid_pair_builds_a_server_config() {
    assert!(server_config(TEST_CERT_PEM, TEST_KEY_PEM).is_ok());
}

#[test]
fn a_valid_pair_builds_an_acceptor() {
    assert!(acceptor(TEST_CERT_PEM, TEST_KEY_PEM).is_ok());
}

/// ⚠️ **`NoCertificate`, not `MalformedCertificate`** — input with no
/// `-----BEGIN`/`-----END` markers at all yields zero PEM sections rather
/// than a parse error (`pem_slice_iter` simply finds nothing), so
/// `server_config`'s own `certs.is_empty()` check is what actually fires.
/// `malformed_certificate_pem_within_valid_markers_is_refused` below is
/// the case that exercises the parse-error path this variant names.
#[test]
fn text_with_no_pem_markers_is_refused_as_no_certificate_not_panicking() {
    assert!(matches!(
        server_config(b"not a certificate", TEST_KEY_PEM),
        Err(TlsConfigError::NoCertificate)
    ));
}

#[test]
fn empty_certificate_pem_is_refused_as_no_certificate() {
    assert!(matches!(
        server_config(b"", TEST_KEY_PEM),
        Err(TlsConfigError::NoCertificate)
    ));
}

/// The parse-error path `MalformedCertificate` actually names: valid PEM
/// markers, invalid base64 between them.
#[test]
fn malformed_base64_within_valid_pem_markers_is_refused_not_panicking() {
    let malformed =
        b"-----BEGIN CERTIFICATE-----\nnot valid base64 at all!!!\n-----END CERTIFICATE-----\n";
    assert!(matches!(
        server_config(malformed, TEST_KEY_PEM),
        Err(TlsConfigError::MalformedCertificate(_))
    ));
}

#[test]
fn malformed_private_key_pem_is_refused_not_panicking() {
    assert!(matches!(
        server_config(TEST_CERT_PEM, b"not a key"),
        Err(TlsConfigError::MalformedPrivateKey(_))
    ));
}

/// ⚠️ **The one test that proves the handshake actually works, over an
/// in-memory duplex pipe** (`tokio::io::duplex` -- no real socket below
/// `bin/oqueue`, this crate's own established precedent for a T1 test).
/// Drives a genuine `rustls` client against this module's server side,
/// with `ring` as the crypto provider on both ends, and confirms bytes
/// written by one side are readable, correctly, on the other -- proving
/// the handshake completed, not merely that both configs constructed
/// without error.
#[tokio::test]
async fn a_real_handshake_completes_and_carries_bytes() {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, ServerName};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let server_acceptor = acceptor(TEST_CERT_PEM, TEST_KEY_PEM).expect("valid pair");

    // The client's own trust store: exactly the one self-signed cert this
    // test's server presents -- a real client would trust a CA instead,
    // but the handshake mechanics this test proves do not depend on which.
    let mut roots = rustls::RootCertStore::empty();
    let cert = CertificateDer::pem_slice_iter(TEST_CERT_PEM)
        .next()
        .expect("one certificate")
        .expect("well-formed");
    roots.add(cert).expect("a self-signed root is addable");
    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let client_connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(client_config));

    let (client_io, server_io) = tokio::io::duplex(4096);

    let server_task = tokio::spawn(async move {
        let mut stream = server_acceptor.accept(server_io).await.expect("handshake");
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.expect("reads");
        assert_eq!(&buf, b"hello");
        stream.write_all(b"world").await.expect("writes");
        stream.flush().await.expect("flushes");
    });

    let server_name = ServerName::try_from("oqueue-test").expect("a valid DNS name");
    let mut client_stream = client_connector
        .connect(server_name, client_io)
        .await
        .expect("client handshake");
    client_stream.write_all(b"hello").await.expect("writes");
    client_stream.flush().await.expect("flushes");
    let mut buf = [0u8; 5];
    client_stream.read_exact(&mut buf).await.expect("reads");
    assert_eq!(&buf, b"world");

    server_task.await.expect("server task does not panic");
}

/// A client that does not trust this certificate is refused, not silently
/// let through -- the property that makes the handshake a real gate rather
/// than a formality neither side actually checks.
#[tokio::test]
async fn a_client_that_does_not_trust_the_certificate_is_refused() {
    use rustls_pki_types::ServerName;

    let server_acceptor = acceptor(TEST_CERT_PEM, TEST_KEY_PEM).expect("valid pair");

    // An empty trust store: nothing this client sees can ever verify.
    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let client_connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(client_config));

    let (client_io, server_io) = tokio::io::duplex(4096);

    let server_task =
        tokio::spawn(async move { server_acceptor.accept(server_io).await.map(|_| ()) });

    let server_name = ServerName::try_from("oqueue-test").expect("a valid DNS name");
    let client_result = client_connector.connect(server_name, client_io).await;
    assert!(
        client_result.is_err(),
        "an untrusted certificate must not be accepted"
    );

    // The server side observes the same failed handshake, from its end.
    let server_result = server_task.await.expect("task does not panic");
    assert!(server_result.is_err());
}
