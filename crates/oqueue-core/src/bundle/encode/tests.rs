//! The two contradictions a `Region` can carry, refused where they are
//! reachable.
//!
//! ⚠️ **Inside the crate because nothing outside it can build one**, exactly as
//! `composite.rs`'s own unit tests are and for the same reason: `Region`'s
//! fields are crate-private and no public constructor can produce a region
//! whose algorithm code and envelope disagree. The contradiction is
//! unreachable through [`BundleBuilder`](crate::BundleBuilder) — and it is
//! precisely the case that would write an object nobody can ever read, so it
//! is tested where it *is* reachable rather than left to a comment.

#![allow(clippy::expect_used)]

use super::encode_footer;
use crate::{
    ByteRange, Dek, Error, KeyId, NonceMinter, ParsedNonce, PartitionId, Redacted, Region,
    RegionAlg, RegionEnvelope, TopicId, WrappedKey,
};

fn envelope() -> RegionEnvelope {
    let nonce = NonceMinter::new(1)
        .expect("in range")
        .next_object()
        .expect("first object")
        .for_region(0)
        .expect("first region");
    RegionEnvelope::new(
        KeyId::new("kek").expect("non-empty"),
        WrappedKey::new(Redacted::new(vec![9_u8; 24])),
        ParsedNonce::decode(*nonce.as_bytes()),
    )
    .expect("a representable envelope")
}

fn region(alg: RegionAlg, envelope: Option<RegionEnvelope>) -> Region {
    Region {
        topic: TopicId::new("orders").expect("a valid topic"),
        partition: PartitionId::new(0).expect("a valid partition"),
        bytes: ByteRange::bounded(0, 32).expect("a valid range"),
        record_count: 1,
        alg,
        envelope,
    }
}

#[test]
fn a_sealed_region_with_no_envelope_is_refused() {
    let mut out = Vec::new();

    assert_eq!(
        encode_footer(&[region(RegionAlg::Aes256Gcm, None)], &mut out),
        Err(Error::RegionEnvelopeMismatch { sealed: true })
    );
}

#[test]
fn an_unsealed_region_carrying_an_envelope_is_refused() {
    let mut out = Vec::new();

    assert_eq!(
        encode_footer(&[region(RegionAlg::None, Some(envelope()))], &mut out),
        Err(Error::RegionEnvelopeMismatch { sealed: false })
    );
}

/// ⚠️ The unbounded-range refusal, which moved here with the encoder.
#[test]
fn an_unbounded_range_is_still_refused() {
    let mut region = region(RegionAlg::None, None);
    region.bytes = ByteRange::Full;
    let mut out = Vec::new();

    assert_eq!(
        encode_footer(&[region], &mut out),
        Err(Error::UnboundedRegion)
    );
}

/// An envelope whose wrapped DEK is empty or wider than the footer's bound is
/// refused where the value is first named, before any payload exists.
#[test]
fn an_unrepresentable_envelope_is_refused_at_construction() {
    let key = KeyId::new("kek").expect("non-empty");
    let nonce = ParsedNonce::decode([0_u8; 12]);

    assert_eq!(
        RegionEnvelope::new(
            key.clone(),
            WrappedKey::new(Redacted::new(Vec::new())),
            nonce
        )
        .err(),
        Some(Error::RegionEnvelopeLength {
            field: "wrapped data encryption key",
            got: 0,
        })
    );
    assert_eq!(
        RegionEnvelope::new(
            key,
            WrappedKey::new(Redacted::new(vec![0_u8; crate::MAX_WRAPPED_DEK_LEN + 1])),
            nonce,
        )
        .err(),
        Some(Error::RegionEnvelopeLength {
            field: "wrapped data encryption key",
            got: crate::MAX_WRAPPED_DEK_LEN + 1,
        })
    );
}

/// ⚠️ A [`Dek`] never reaches the footer — only its *wrapped* form does. This
/// asserts the shape rather than any behaviour: `RegionEnvelope` has no
/// constructor and no field that takes plaintext key material.
#[test]
fn the_envelope_carries_no_plaintext_key_material() {
    let dek = Dek::new([3_u8; 32]);
    let envelope = envelope();

    assert!(!format!("{envelope:?}").contains("Redacted(["));
    assert_eq!(format!("{dek:?}"), "Dek(<redacted>)");
}
