//! The unencrypted path is a configuration that refuses, not one that pretends.

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use oqueue_core::{Error, KeyId, KeyProvider, Redacted, WrappedKey};
use oqueue_crypto::NoOpKeyProvider;
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// ⚠️ No async runtime — see ADR-0002 and `M0.16`'s floor. Correct only because
/// every future here completes on its first poll.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}

fn key_id() -> KeyId {
    KeyId::new("arn:aws:kms:eu-west-1:123456789012:key/abcd").expect("non-empty")
}

/// ⚠️ The property that matters: it never returns a *successful* wrap. An
/// identity provider would write a plaintext DEK into object storage looking
/// exactly like a real one.
#[test]
fn wrapping_is_refused_rather_than_passed_through() {
    let provider = NoOpKeyProvider::new();
    let dek = Redacted::new(vec![0xde, 0xad, 0xbe, 0xef]);

    // ⚠️ Asserted on the error, not with `assert_eq!` on the `Result` —
    // `WrappedKey` is deliberately not `PartialEq` (ADR-0006 guarantee 6: a
    // real KMS may return different ciphertext for the same DEK each call).
    let err = block_on(provider.wrap(&key_id(), &dek)).expect_err("wrap must refuse");

    assert_eq!(err, Error::EncryptionDisabled);
}

/// And the same on the way back, so a deployment cannot be switched off and
/// still read what it wrote encrypted.
#[test]
fn unwrapping_is_refused() {
    let provider = NoOpKeyProvider::new();
    let wrapped = WrappedKey::new(Redacted::new(vec![1, 2, 3]));

    assert_eq!(
        block_on(provider.unwrap(&key_id(), &wrapped)),
        Err(Error::EncryptionDisabled)
    );
}

/// Its error names the misconfiguration and no material.
#[test]
fn the_refusal_names_no_key_material() {
    let rendered = format!("{}", Error::EncryptionDisabled);
    assert_eq!(rendered, "encryption is not enabled on this deployment");
}

/// It is usable as a trait object, like every other implementor of the seam.
#[test]
fn the_no_op_provider_is_dyn_compatible() {
    let provider: std::sync::Arc<dyn KeyProvider> = std::sync::Arc::new(NoOpKeyProvider::new());
    let dek = Redacted::new(vec![1]);
    assert_eq!(
        block_on(provider.wrap(&key_id(), &dek)).expect_err("wrap must refuse"),
        Error::EncryptionDisabled
    );
}
