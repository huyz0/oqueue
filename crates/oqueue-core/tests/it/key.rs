//! The `KeyProvider` seam and its fake.

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use oqueue_core::{Dek, Error, FakeKeyProvider, KeyId, KeyProvider, Redacted};
use std::future::Future;
use std::task::{Context, Poll, Waker};
use zeroize::{Zeroize, ZeroizeOnDrop};

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

// ── `Dek`: M8.1, security.md rules 6-8 ──────────────────────────────────────

/// ⚠️ **What this proves and what it does not.** It calls [`Zeroize::zeroize`]
/// — the same function `Dek`'s `Drop` body calls, and the only one — and
/// asserts the bytes it owned are now zero. It therefore proves the wiping
/// *operation* is real and reaches all 32 bytes.
///
/// ⚠️ It does **not** observe memory after the drop. Reading a dropped value's
/// storage is undefined behaviour in safe Rust and unreachable without
/// `unsafe`, which `AGENTS.md` non-negotiable 7 forbids in this crate, so no
/// test here can watch the freed bytes. The link from "zeroize wipes" to
/// "drop wipes" is carried by two other things instead: the `Drop` impl's one
/// statement, and `assert_dek_is_zeroize_on_drop` below, which fails to
/// *compile* if `Dek` stops implementing [`ZeroizeOnDrop`].
#[test]
fn zeroizing_a_dek_leaves_no_byte_of_key_material_in_the_bytes_it_owned() {
    let mut dek = Dek::new([0xA5; 32]);
    assert_eq!(dek.expose(), &[0xA5; 32]);

    dek.zeroize();

    assert_eq!(dek.expose(), &[0u8; 32]);
}

/// The compile-time half of the claim above: `Dek` promises wiping on drop in
/// the type system, not only in a comment.
#[test]
fn assert_dek_is_zeroize_on_drop() {
    const fn requires_zeroize_on_drop<T: ZeroizeOnDrop>() {}
    requires_zeroize_on_drop::<Dek>();
}

/// A `Dek` built from a correctly sized slice carries those bytes.
#[test]
fn a_dek_from_a_thirty_two_byte_slice_holds_those_bytes() {
    let bytes: Vec<u8> = (0..32).collect();
    let dek = Dek::from_slice(&bytes).expect("32 bytes");
    assert_eq!(&dek.expose()[..], &bytes[..]);
    assert_eq!(oqueue_core::DEK_BYTES, 32);
}

/// Wrong lengths are refused, and the refusal names the length only.
#[test]
fn a_dek_from_a_wrong_length_slice_is_refused_without_naming_the_bytes() {
    for len in [0_usize, 16, 31, 33, 64] {
        let bytes = vec![0xEE_u8; len];
        let err = Dek::from_slice(&bytes).expect_err("wrong length");
        assert_eq!(err, Error::DekLength { got: len });
        assert_eq!(
            format!("{err}"),
            format!("a data encryption key must be 32 bytes, got {len}")
        );
        assert_eq!(format!("{err:?}"), format!("DekLength {{ got: {len} }}"));
    }
}

/// ⚠️ Pinned, not screened — `redaction.rs`'s header says why.
#[test]
fn dek_renderings_are_exactly_these_and_nothing_else() {
    let dek = Dek::new([0xDE; 32]);

    assert_eq!(format!("{dek:?}"), "Dek(<redacted>)");
    assert_eq!(format!("{dek:#?}"), "Dek(<redacted>)");
    assert_eq!(format!("{dek:x?}"), "Dek(<redacted>)");
    assert_eq!(format!("{dek:X?}"), "Dek(<redacted>)");
    assert_eq!(format!("{dek:+?}"), "Dek(<redacted>)");
    assert_eq!(format!("{dek:>40?}"), "Dek(<redacted>)");
}

/// Constant-time equality answers the ordinary questions correctly.
#[test]
fn constant_time_equality_answers_the_same_questions_ordinary_equality_would() {
    let a = Dek::new([7; 32]);
    let b = Dek::new([7; 32]);
    assert!(a.ct_eq(&b));
    assert!(a.ct_eq(&a));

    // Differing in the first byte, and in the last: neither position is
    // special to the comparison.
    let mut first = [7_u8; 32];
    first[0] = 8;
    let mut last = [7_u8; 32];
    last[31] = 8;
    assert!(!a.ct_eq(&Dek::new(first)));
    assert!(!a.ct_eq(&Dek::new(last)));
}

#[test]
fn constant_time_byte_comparison_answers_against_a_borrowed_slice() {
    let secret = Redacted::new("secret".to_owned());

    assert!(secret.ct_eq_bytes(b"secret"));
    assert!(!secret.ct_eq_bytes(b"secrex"));
    assert!(!secret.ct_eq_bytes(b"x"));
}

/// `Redacted`'s own equality, now constant-time, still answers correctly —
/// including for unequal lengths, which it reports as unequal.
#[test]
fn redacted_equality_answers_correctly_for_equal_and_unequal_contents() {
    let a = Redacted::new(vec![1_u8, 2, 3]);
    assert_eq!(a, Redacted::new(vec![1_u8, 2, 3]));
    assert_ne!(a, Redacted::new(vec![1_u8, 2, 4]));
    assert_ne!(a, Redacted::new(vec![1_u8, 2, 3, 4]));
    assert_ne!(a, Redacted::new(Vec::new()));
    assert_eq!(
        Redacted::new(String::from("s")),
        Redacted::new(String::from("s"))
    );
}

/// `Redacted` wipes on request. ⚠️ Same honest limit as the `Dek` test above:
/// this proves the operation, not a post-drop observation — and `Redacted`
/// deliberately does **not** wipe on drop, which its own documentation states.
#[test]
fn zeroizing_a_redacted_vec_empties_it() {
    let mut r = Redacted::new(vec![0xA5_u8; 32]);
    r.zeroize();
    assert!(
        r.expose().is_empty(),
        "Vec<u8>::zeroize clears as well as wipes"
    );
}
