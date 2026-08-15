//! The `KeyProvider` seam and its fake.

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use oqueue_core::{Error, FakeKeyProvider, KeyId, KeyProvider, Redacted};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// ⚠️ No async runtime. Correct only because every future here completes on its
/// first poll — the fake does no I/O and registers no waker.
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

fn kid(s: &str) -> KeyId {
    KeyId::new(s).expect("non-empty")
}

const DEK: &[u8] = &[0x9f, 0x2b, 0x1c, 0x8d, 0x00, 0xff];

/// The round trip: unwrap(wrap(k)) is k, under the same key id.
#[test]
fn a_key_wrapped_and_unwrapped_under_one_id_comes_back_unchanged() {
    let provider = FakeKeyProvider::new();
    let id = kid("arn:aws:kms:eu-west-1:1:key/a");
    let dek = Redacted::new(DEK.to_vec());

    let wrapped = block_on(provider.wrap(&id, &dek)).expect("wrap succeeds");
    let unwrapped = block_on(provider.unwrap(&id, &wrapped)).expect("unwrap succeeds");

    assert_eq!(unwrapped.expose(), DEK);
}

/// ⚠️ The guarantee worth having in a fake: a key wrapped under one id does not
/// unwrap under another. Without it, a test could not catch the mistake this
/// seam most needs to prevent.
#[test]
fn a_key_does_not_unwrap_under_a_different_id() {
    let provider = FakeKeyProvider::new();
    let a = kid("arn:aws:kms:eu-west-1:1:key/a");
    let b = kid("arn:aws:kms:eu-west-1:1:key/b");
    let dek = Redacted::new(DEK.to_vec());

    let wrapped = block_on(provider.wrap(&a, &dek)).expect("wrap succeeds");

    assert_eq!(
        block_on(provider.unwrap(&b, &wrapped)),
        Err(Error::SecretRejected {
            context: "unwrapping a data encryption key",
            key_id: b,
        })
    );
}

/// ⚠️ FR-44: no formatting of a wrapped key reaches its bytes, at any specifier.
#[test]
fn a_wrapped_key_never_renders_its_bytes() {
    let provider = FakeKeyProvider::new();
    let id = kid("arn:aws:kms:eu-west-1:1:key/a");
    let dek = Redacted::new(DEK.to_vec());
    let wrapped = block_on(provider.wrap(&id, &dek)).expect("wrap succeeds");

    assert_eq!(format!("{wrapped:?}"), "WrappedKey(Redacted(<redacted>))");
    assert_eq!(
        format!("{wrapped:#?}"),
        "WrappedKey(\n    Redacted(<redacted>),\n)"
    );
    assert_eq!(format!("{wrapped:x?}"), "WrappedKey(Redacted(<redacted>))");
    let rendered = format!("{wrapped:?}{wrapped:#?}{wrapped:x?}");
    assert!(!rendered.contains(&format!("{DEK:?}")), "leak: {rendered}");
}

/// ⚠️ And the error raised about a rejected key names the id, never material.
#[test]
fn the_rejection_error_names_the_key_id_and_no_material() {
    let provider = FakeKeyProvider::new();
    let a = kid("arn:aws:kms:eu-west-1:1:key/a");
    let b = kid("arn:aws:kms:eu-west-1:1:key/b");
    let wrapped = block_on(provider.wrap(&a, &Redacted::new(DEK.to_vec()))).expect("wrap");

    let err = block_on(provider.unwrap(&b, &wrapped)).expect_err("wrong id is rejected");
    let rendered = format!("{err} {err:?} {err:#?}");

    assert!(
        rendered.contains("key/b"),
        "the id an operator needs is absent"
    );
    assert!(
        !rendered.contains(&format!("{DEK:?}")),
        "material leaked: {rendered}"
    );
}

/// A key id must name something.
#[test]
fn an_empty_key_id_is_refused() {
    assert_eq!(KeyId::new(""), Err(Error::EmptyKeyId));
}

/// The seam is usable as a trait object, as the composition root needs.
#[test]
fn the_seam_is_dyn_compatible() {
    let provider: std::sync::Arc<dyn KeyProvider> = std::sync::Arc::new(FakeKeyProvider::new());
    let id = kid("arn:aws:kms:eu-west-1:1:key/a");
    let wrapped = block_on(provider.wrap(&id, &Redacted::new(DEK.to_vec()))).expect("wrap");
    assert_eq!(
        block_on(provider.unwrap(&id, &wrapped))
            .expect("unwrap")
            .expose(),
        DEK
    );
}

/// ⚠️ A regression test for a real aliasing bug review found: with a
/// null-terminated tag, a key wrapped under `"a\0"` unwrapped under `"a"` —
/// succeeding with a corrupted DEK, which is worse than failing. The tag is
/// length-prefixed now.
#[test]
fn a_key_id_that_is_a_prefix_of_another_does_not_alias() {
    let provider = FakeKeyProvider::new();
    let short = kid("a");
    let long = kid("a\0");
    let dek = Redacted::new(DEK.to_vec());

    let wrapped_under_long = block_on(provider.wrap(&long, &dek)).expect("wrap");
    assert_eq!(
        block_on(provider.unwrap(&short, &wrapped_under_long)),
        Err(Error::SecretRejected {
            context: "unwrapping a data encryption key",
            key_id: short.clone(),
        })
    );

    let wrapped_under_short = block_on(provider.wrap(&short, &dek)).expect("wrap");
    assert!(block_on(provider.unwrap(&long, &wrapped_under_short)).is_err());
}
