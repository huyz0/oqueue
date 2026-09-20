//! FR-41's portable provider seam: one test body, two KMS API simulations.

#![allow(clippy::expect_used)]

use crate::support::block_on;
use oqueue_core::{BoxFuture, DEK_BYTES, Error, KeyId, KeyProvider, Redacted, Result};
use oqueue_crypto::{AwsKmsApi, AwsKmsProvider, GcpCloudKmsApi, GcpKmsProvider};

const AWS_ENVELOPE: &[u8] = b"aws-kms-encrypt-v1\0";
const GCP_ENVELOPE: &[u8] = b"gcp-cloud-kms-encrypt-v1\0";

/// An in-process AWS KMS `Encrypt`/`Decrypt` simulation.
#[derive(Debug, Default)]
struct SimulatedAwsKms;

impl AwsKmsApi for SimulatedAwsKms {
    fn encrypt<'a>(
        &'a self,
        key_id: &'a str,
        plaintext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move { simulated_encrypt(AWS_ENVELOPE, key_id, plaintext) })
    }

    fn decrypt<'a>(
        &'a self,
        key_id: &'a str,
        ciphertext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move { simulated_decrypt(AWS_ENVELOPE, key_id, ciphertext) })
    }
}

/// An in-process GCP Cloud KMS `encrypt`/`decrypt` simulation.
#[derive(Debug, Default)]
struct SimulatedGcpCloudKms;

impl GcpCloudKmsApi for SimulatedGcpCloudKms {
    fn encrypt<'a>(
        &'a self,
        key_id: &'a str,
        plaintext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move { simulated_encrypt(GCP_ENVELOPE, key_id, plaintext) })
    }

    fn decrypt<'a>(
        &'a self,
        key_id: &'a str,
        ciphertext: &'a [u8],
    ) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move { simulated_decrypt(GCP_ENVELOPE, key_id, ciphertext) })
    }
}

/// The simulation models API framing and key-domain binding, not cryptography.
/// Real AWS/GCP calls are M15's evidence; this test proves both adapters feed
/// the same `KeyProvider` contract correctly.
fn simulated_encrypt(prefix: &[u8], key_id: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let key_id = key_id.as_bytes();
    let key_len = u16::try_from(key_id.len()).map_err(|_| Error::Permanent)?;
    let mut ciphertext = prefix.to_vec();
    ciphertext.extend_from_slice(&key_len.to_be_bytes());
    ciphertext.extend_from_slice(key_id);
    ciphertext.extend_from_slice(plaintext);
    Ok(ciphertext)
}

fn simulated_decrypt(prefix: &[u8], key_id: &str, ciphertext: &[u8]) -> Result<Vec<u8>> {
    let Some(rest) = ciphertext.strip_prefix(prefix) else {
        return Err(Error::SecretRejected {
            context: "decrypting a data encryption key",
            key_id: KeyId::new(key_id).expect("the test key id is non-empty"),
        });
    };
    let Some((key_len, rest)) = rest.split_first_chunk::<2>() else {
        return Err(Error::SecretRejected {
            context: "decrypting a data encryption key",
            key_id: KeyId::new(key_id).expect("the test key id is non-empty"),
        });
    };
    let key_len = usize::from(u16::from_be_bytes(*key_len));
    let Some((ciphertext_key, plaintext)) = rest.split_at_checked(key_len) else {
        return Err(Error::SecretRejected {
            context: "decrypting a data encryption key",
            key_id: KeyId::new(key_id).expect("the test key id is non-empty"),
        });
    };
    if ciphertext_key != key_id.as_bytes() {
        return Err(Error::SecretRejected {
            context: "decrypting a data encryption key",
            key_id: KeyId::new(key_id).expect("the test key id is non-empty"),
        });
    }
    Ok(plaintext.to_vec())
}

fn assert_round_trip(provider: &dyn KeyProvider) {
    let key_id =
        KeyId::new("projects/p/locations/l/keyRings/r/cryptoKeys/orders").expect("non-empty");
    let plaintext = Redacted::new(
        (0..DEK_BYTES)
            .map(|byte| u8::try_from(byte).expect("DEK_BYTES fits in a byte"))
            .collect(),
    );
    let wrapped = block_on(provider.wrap(&key_id, &plaintext)).expect("wrap succeeds");
    let opened = block_on(provider.unwrap(&key_id, &wrapped)).expect("unwrap succeeds");
    assert_eq!(opened.expose(), plaintext.expose());

    let other_key =
        KeyId::new("projects/p/locations/l/keyRings/r/cryptoKeys/payments").expect("non-empty");
    assert!(
        block_on(provider.unwrap(&other_key, &wrapped)).is_err(),
        "a wrapped DEK must not cross key domains"
    );
}

#[test]
fn both_kms_providers_round_trip_through_one_seam() {
    let providers: [Box<dyn KeyProvider>; 2] = [
        Box::new(AwsKmsProvider::new(SimulatedAwsKms)),
        Box::new(GcpKmsProvider::new(SimulatedGcpCloudKms)),
    ];

    for provider in providers {
        assert_round_trip(provider.as_ref());
    }
}
