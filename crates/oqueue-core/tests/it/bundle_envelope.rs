//! The envelope a sealed region carries in the footer (`M8.4`).
//!
//! ⚠️ **Two halves, and the second is the one that is easy to lose.** The first
//! is that a sealed region round-trips: what a reader parses back is the key
//! id, wrapped DEK and nonce the writer put there. The second is that a region
//! with `alg = 0` encodes **exactly** what it encoded before this row — pinned
//! against a literal rather than against the code that produces it, because a
//! round-trip test passes just as happily when both halves move together, and
//! what "byte-identical to what the tree writes today" means is that objects
//! written by the previous build still read.
//!
//! ⚠️ **The adversarial cases here are the ones that still *parse*.** A footer
//! whose envelope is truncated fails on any check; a footer whose `alg` byte
//! was flipped from `1` to `0` is well-formed in every field and simply means
//! something else, and that is the edit `M8.12` exists for. What this file can
//! assert is that the *parse* refuses it, because the envelope's bytes are
//! then read as the next region's fields.

#![allow(clippy::expect_used)]

use oqueue_core::{
    BundleBuilder, ByteRange, CoordinatorEpoch, Error, KeyId, MAX_WRAPPED_DEK_LEN, NonceMinter,
    ParsedNonce, PartitionId, PushedRecords, Redacted, Region, RegionAlg, RegionEnvelope,
    SealedRegion, TopicId, WrappedKey, WriterEpoch, parse_footer,
};

use crate::bundle_footer::four_topics;

const WRAPPED: &[u8] = b"a wrapped data encryption key, or near enough";

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn envelope(key: &str, region_index: u32) -> RegionEnvelope {
    let mut source = NonceMinter::new(WriterEpoch::from_coordinator_epoch(CoordinatorEpoch::new(
        7,
    )))
    .expect("in range")
    .next_object()
    .expect("first object");
    for index in 0..=region_index {
        let nonce = source.for_region(index).expect("in order");
        if index == region_index {
            return RegionEnvelope::new(
                KeyId::new(key.to_owned()).expect("non-empty"),
                WrappedKey::new(Redacted::new(WRAPPED.to_vec())),
                ParsedNonce::decode(*nonce.as_bytes()),
            )
            .expect("a representable envelope");
        }
    }
    unreachable!("the loop returns at `region_index`")
}

fn sealed_bytes(marker: u8) -> Vec<u8> {
    // Not real ciphertext: this file is about the footer, and `oqueue-core`
    // performs no cryptography. What matters is that the bytes are opaque to
    // the format and that the region's range covers all of them.
    vec![marker; 48]
}

/// `mixed()`, but with a wrapped key of exactly `len` bytes — what the
/// bound-edge test needs, since the envelope helper's key is a fixed string.
fn mixed_with_wrapped(len: usize) -> BundleBuilder {
    let mut source = NonceMinter::new(WriterEpoch::from_coordinator_epoch(CoordinatorEpoch::new(
        9,
    )))
    .expect("in range")
    .next_object()
    .expect("first object");
    let nonce = source.for_region(0).expect("in order");
    let envelope = RegionEnvelope::new(
        KeyId::new("arn:aws:kms:eu-west-1:1:key/abc".to_owned()).expect("non-empty"),
        WrappedKey::new(Redacted::new(vec![b'w'; len])),
        ParsedNonce::decode(*nonce.as_bytes()),
    )
    .expect("a representable envelope");
    let mut bundle = BundleBuilder::new();
    bundle
        .push_sealed(
            topic("orders"),
            PartitionId::new(0).expect("a valid partition"),
            PushedRecords {
                count: 3,
                producer: None,
            },
            SealedRegion {
                bytes: &sealed_bytes(b'z'),
                alg: RegionAlg::Aes256Gcm,
                envelope,
            },
        )
        .expect("a non-empty sealed region");
    bundle
}

/// One sealed region, and one unsealed one, in a single object.
///
/// ⚠️ **The format permits both, and `M8.6` is what will forbid it** — no
/// object may mix two key domains, which is a routing decision the flush
/// planner makes. This row is about what the bytes can express, so the mixed
/// object is built here deliberately rather than by accident.
fn mixed() -> BundleBuilder {
    let mut bundle = BundleBuilder::new();
    bundle
        .push_sealed(
            topic("orders"),
            PartitionId::new(0).expect("a valid partition"),
            PushedRecords {
                count: 3,
                producer: None,
            },
            SealedRegion {
                bytes: &sealed_bytes(b'z'),
                alg: RegionAlg::Aes256Gcm,
                envelope: envelope("arn:aws:kms:eu-west-1:1:key/abc", 0),
            },
        )
        .expect("a non-empty sealed region");
    bundle
        .push(
            topic("telemetry"),
            PartitionId::new(1).expect("a valid partition"),
            PushedRecords {
                count: 2,
                producer: None,
            },
            b"plain record batches",
        )
        .expect("a non-empty region");
    bundle
}

#[test]
fn a_sealed_region_round_trips_through_the_footer() {
    let bundle = {
        let mut bundle = BundleBuilder::new();
        bundle
            .push_sealed(
                topic("orders"),
                PartitionId::new(4).expect("a valid partition"),
                PushedRecords {
                    count: 9,
                    producer: None,
                },
                SealedRegion {
                    bytes: &sealed_bytes(b'x'),
                    alg: RegionAlg::Aes256Gcm,
                    envelope: envelope("kek-1", 0),
                },
            )
            .expect("a non-empty sealed region");
        bundle
    };
    let object = bundle.seal().expect("regions were pushed");

    let read = parse_footer(object.payload(), object.payload().len() as u64).expect("it parses");
    assert_eq!(read, object.regions(), "field for field, envelope included");

    let region = read.first().expect("one region");
    let envelope = region.envelope().expect("a sealed region carries one");
    assert_eq!(region.alg(), RegionAlg::Aes256Gcm);
    assert_eq!(envelope.key_id().as_str(), "kek-1");
    assert_eq!(envelope.wrapped_dek().as_redacted().expose(), WRAPPED);
    assert_eq!(
        envelope.nonce().as_bytes(),
        &[0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 0, 0],
        "writer epoch 7, object 0, region 0"
    );
    assert_eq!(
        region.bytes(),
        ByteRange::bounded(0, 48).expect("a valid range"),
        "the range covers the ciphertext and its tag"
    );
}

#[test]
fn an_object_may_hold_a_sealed_region_beside_an_unsealed_one() {
    let object = mixed().seal().expect("regions were pushed");

    let read = parse_footer(object.payload(), object.payload().len() as u64).expect("it parses");
    assert_eq!(read, object.regions());
    assert!(
        read[0].envelope().is_some(),
        "the sealed region carries one"
    );
    assert_eq!(read[0].alg(), RegionAlg::Aes256Gcm);
    assert!(
        read[1].envelope().is_none(),
        "and the unsealed one does not"
    );
    assert_eq!(read[1].alg(), RegionAlg::None);
}

/// ⚠️ **The pin, against a literal derived from the format rather than from a
/// run.** These 217 bytes are `four_topics()`'s object as the *documented*
/// shape produces it — `u16 name_len ‖ name ‖ u32 partition ‖ u32
/// record_count ‖ u64 offset ‖ u64 length ‖ u8 alg`, then a nine-byte trailer
/// of `u32 count ‖ u8 version ‖ u32 footer_len`. A round trip cannot see the
/// encoder and the parser drifting together; this can, and an object written
/// by the previous build is exactly a literal like this one.
#[test]
fn an_object_with_no_sealed_region_is_byte_identical_to_what_the_tree_wrote_before() {
    let object = four_topics().seal().expect("regions were pushed");

    assert_eq!(hex(object.payload()), UNSEALED_VECTOR);
    assert_eq!(
        object.payload()[object.payload().len() - 5],
        1,
        "the version byte did not move: the envelope is discriminated by `alg`"
    );
}

/// `four_topics()`, as an object.
const UNSEALED_VECTOR: &str = "6161616161616161616161616161616162626262626262626262626262626262626363636363636363636363636363636363636464646464646464646464646464646464646400066f72646572730000000000000001000000000000000000000000000000100000087061796d656e7473000000000000000200000000000000100000000000000011000009736869706d656e747300000000000000030000000000000021000000000000001200000772657475726e730000000000000004000000000000003300000000000000130000000004010000008a";

/// A truncated envelope is refused rather than read short, at every prefix.
#[test]
fn a_truncated_envelope_is_refused() {
    let object = mixed().seal().expect("regions were pushed");
    let bytes = object.payload();

    for cut in 1..40 {
        let mut torn = bytes.to_vec();
        let trailer_at = torn.len() - 9;
        torn.drain(trailer_at - cut..trailer_at);
        // The trailer still claims the old footer length, so this is what a
        // torn object looks like from the parser's side.
        assert!(
            parse_footer(&torn, torn.len() as u64).is_err(),
            "a footer {cut} bytes shorter than its trailer claims is refused"
        );
    }
}

/// A length field claiming more than the object holds is refused before
/// anything is allocated for it.
#[test]
fn an_envelope_length_claiming_more_than_the_object_holds_is_refused() {
    let object = mixed().seal().expect("regions were pushed");
    let at = wrapped_len_at(object.payload());

    for claimed in [u32::MAX, 1 << 20, 9000] {
        let mut forged = object.payload().to_vec();
        forged[at..at + 4].copy_from_slice(&claimed.to_be_bytes());
        assert!(
            matches!(
                parse_footer(&forged, forged.len() as u64),
                Err(Error::MalformedBundleFooter { .. })
            ),
            "a wrapped-key length of {claimed} is refused, not allocated"
        );
    }
}

/// ⚠️ **Zero too.** An envelope naming no wrapped key at all is as malformed as
/// one naming four gigabytes of it, and the bound that refuses the second is
/// the one that must refuse the first.
#[test]
fn an_envelope_naming_an_empty_wrapped_key_is_refused() {
    let object = mixed().seal().expect("regions were pushed");
    let at = wrapped_len_at(object.payload());
    let mut forged = object.payload().to_vec();
    forged[at..at + 4].copy_from_slice(&0_u32.to_be_bytes());

    assert!(matches!(
        parse_footer(&forged, forged.len() as u64),
        Err(Error::MalformedBundleFooter { .. })
    ));
}

/// ⚠️ **Both edges of the wrapped-key bound, pinned** (`M8.4`'s mutants): the
/// empty case above and the over-long case here are one `||` apart in the
/// parser, and a bound that read `>=` would refuse a footer this build would
/// itself have written.
#[test]
fn a_wrapped_key_past_the_bound_is_refused_and_one_at_it_is_not() {
    let object = mixed().seal().expect("regions were pushed");
    let at = wrapped_len_at(object.payload());

    let mut forged = object.payload().to_vec();
    let past = u32::try_from(MAX_WRAPPED_DEK_LEN + 1).expect("fits");
    forged[at..at + 4].copy_from_slice(&past.to_be_bytes());
    assert!(
        matches!(
            parse_footer(&forged, forged.len() as u64),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "one byte past the bound is refused"
    );

    // And a footer whose wrapped key is exactly the bound parses, so the
    // refusal is of what is past it rather than of the bound itself.
    let at_bound = mixed_with_wrapped(MAX_WRAPPED_DEK_LEN)
        .seal()
        .expect("regions were pushed");
    let regions =
        parse_footer(at_bound.payload(), at_bound.payload().len() as u64).expect("parses");
    let envelope = regions
        .iter()
        .find_map(Region::envelope)
        .expect("a sealed region");
    assert_eq!(
        envelope.wrapped_dek().as_redacted().expose().len(),
        MAX_WRAPPED_DEK_LEN,
        "the bound itself is writable and readable"
    );
}

/// A key id of the wrong size moves every field after it, so the footer no
/// longer describes itself.
#[test]
fn a_key_id_of_the_wrong_size_is_refused() {
    let object = mixed().seal().expect("regions were pushed");
    let at = key_len_at(object.payload());

    for claimed in [0_u16, 1, 7, u16::MAX] {
        let mut forged = object.payload().to_vec();
        forged[at..at + 2].copy_from_slice(&claimed.to_be_bytes());
        assert!(
            parse_footer(&forged, forged.len() as u64).is_err(),
            "a key-id length of {claimed} is refused"
        );
    }
}

/// ⚠️ **The `M8.12` edit, as far as this row can reach it.** Clearing the `alg`
/// byte of a sealed region is what turns ciphertext into "stored as written",
/// and the *parse* refuses it here only because the envelope's bytes are then
/// read as the next region's fields. That is a consequence of the layout, not
/// a guarantee about the byte — which is why `M8.12` derives sealed-ness from
/// the topic's key domain rather than from the footer.
#[test]
fn a_sealed_region_whose_alg_says_none_is_refused_by_the_parser() {
    let object = mixed().seal().expect("regions were pushed");
    let at = alg_at(object.payload());
    let mut forged = object.payload().to_vec();
    assert_eq!(forged[at], RegionAlg::Aes256Gcm.code(), "the alg byte");
    forged[at] = RegionAlg::None.code();

    assert!(parse_footer(&forged, forged.len() as u64).is_err());
}

/// And the other direction: an unsealed region with envelope bytes after it.
#[test]
fn an_unsealed_region_carrying_envelope_bytes_is_refused() {
    let object = four_topics().seal().expect("regions were pushed");
    let mut forged = object.payload().to_vec();
    let trailer_at = forged.len() - 9;
    // Envelope-shaped bytes spliced in after the first region's `alg` byte,
    // with the footer length grown to match — so the only thing wrong is that
    // an `alg = 0` region does not carry them.
    let extra: Vec<u8> = [0_u8, 3]
        .into_iter()
        .chain(*b"kek")
        .chain([0, 0, 0, 4])
        .chain(*b"wrap")
        .chain([0_u8; 12])
        .collect();
    let after_alg = alg_at(&forged) + 1;
    forged.splice(after_alg..after_alg, extra.iter().copied());
    let footer_len_at = trailer_at + extra.len() + 5;
    let grown = u32::from_be_bytes([
        forged[footer_len_at],
        forged[footer_len_at + 1],
        forged[footer_len_at + 2],
        forged[footer_len_at + 3],
    ]) + u32::try_from(extra.len()).expect("a small splice");
    forged[footer_len_at..footer_len_at + 4].copy_from_slice(&grown.to_be_bytes());

    assert!(
        parse_footer(&forged, forged.len() as u64).is_err(),
        "envelope bytes after an `alg = 0` region are read as the next region's"
    );
}

/// A region with more regions than the footer holds still refuses, with the
/// envelope in the way — the existing bound, re-asserted on the new shape.
#[test]
fn a_region_count_beyond_the_footer_is_still_refused() {
    let object = mixed().seal().expect("regions were pushed");
    let mut forged = object.payload().to_vec();
    let count_at = forged.len() - 9;
    forged[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());

    assert!(matches!(
        parse_footer(&forged, forged.len() as u64),
        Err(Error::MalformedBundleFooter { .. })
    ));
}

// ── Where the first region's fields are ─────────────────────────────────────
//
// ⚠️ Computed from the footer's own shape rather than hardcoded, so these
// helpers move with the format instead of silently pointing at the wrong byte.

fn footer_at(payload: &[u8]) -> usize {
    let trailer_at = payload.len() - 9;
    let footer_len = usize::try_from(u32::from_be_bytes([
        payload[trailer_at + 5],
        payload[trailer_at + 6],
        payload[trailer_at + 7],
        payload[trailer_at + 8],
    ]))
    .expect("a small footer");
    trailer_at - footer_len
}

/// The first region's algorithm byte.
fn alg_at(payload: &[u8]) -> usize {
    let at = footer_at(payload);
    let name_len = usize::from(u16::from_be_bytes([payload[at], payload[at + 1]]));
    at + 2 + name_len + 4 + 4 + 8 + 8
}

/// The first region's key-id length field, which follows the algorithm byte.
fn key_len_at(payload: &[u8]) -> usize {
    alg_at(payload) + 1
}

/// The first region's wrapped-key length field.
fn wrapped_len_at(payload: &[u8]) -> usize {
    let at = key_len_at(payload);
    let key_len = usize::from(u16::from_be_bytes([payload[at], payload[at + 1]]));
    at + 2 + key_len
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// ⚠️ A `Region` from a parse and a `Region` from the builder compare equal
/// **including the wrapped key's bytes**, which `WrappedKey` itself refuses to
/// answer — `RegionEnvelope`'s own `PartialEq` says why that is the right
/// question for a footer and the wrong one for a key.
#[test]
fn equality_covers_the_envelope_rather_than_ignoring_it() {
    let object = mixed().seal().expect("regions were pushed");
    let read = parse_footer(object.payload(), object.payload().len() as u64).expect("it parses");
    let mine: &Region = &read[0];

    assert_eq!(mine, &object.regions()[0]);
    let mut different = object.payload().to_vec();
    let at = wrapped_len_at(&different) + 4;
    different[at] ^= 1;
    let other = parse_footer(&different, different.len() as u64).expect("still well-formed");
    assert_ne!(
        &other[0], mine,
        "a changed wrapped key is a changed footer, and equality sees it"
    );
}
