//! `flex.rs`'s tests, split out at the 500-line limit — `oqueue-codec/src/
//! fetch/tests.rs`'s own precedent: the module boundary the line limit
//! points at is the primitives themselves versus the tests that pin them,
//! not any one test being cuttable. `M9.3`'s `read_string`/`read_bytes`/
//! `put_bytes` additions are what pushed this file over the line.

#![allow(clippy::expect_used)]

use super::*;

#[test]
fn compact_string_round_trips_including_null_and_empty() {
    for value in [Some("hello"), Some(""), None] {
        let mut buf = Vec::new();
        put_compact_nullable_string(&mut buf, value);
        let mut cur = Cursor::new(&buf);
        let got = read_compact_nullable_string(&mut cur).expect("decodes");
        assert_eq!(got, value);
        assert_eq!(cur.remaining(), 0, "the whole field is consumed");
    }
}

#[test]
fn a_non_null_compact_string_rejects_the_null_encoding() {
    let mut buf = Vec::new();
    put_compact_nullable_string(&mut buf, None);
    let mut cur = Cursor::new(&buf);
    assert!(matches!(
        read_compact_string(&mut cur),
        Err(DecodeError::UnexpectedNull { .. })
    ));
}

#[test]
fn compact_bytes_round_trip() {
    for value in [Some(&b"\x00\x01\xff"[..]), Some(&b""[..]), None] {
        let mut buf = Vec::new();
        put_compact_nullable_bytes(&mut buf, value);
        let mut cur = Cursor::new(&buf);
        assert_eq!(
            read_compact_nullable_bytes(&mut cur).expect("decodes"),
            value
        );
    }
}

#[test]
fn a_length_past_the_input_is_rejected_before_any_read() {
    // uvarint(1000 + 1) claims a 1000-byte string in a 3-byte buffer.
    let mut buf = Vec::new();
    put_compact_len(&mut buf, Some(1000));
    let mut cur = Cursor::new(&buf);
    assert!(matches!(
        read_compact_nullable_string(&mut cur),
        Err(DecodeError::LengthOutOfBounds { .. })
    ));
}

#[test]
fn an_array_length_is_bounded_by_remaining() {
    // The DoS shape in miniature: a huge count in a tiny buffer.
    let mut buf = Vec::new();
    put_compact_array_len(&mut buf, Some(2_000_000_000));
    let mut cur = Cursor::new(&buf);
    assert!(
        matches!(
            read_compact_array_len(&mut cur),
            Err(DecodeError::LengthOutOfBounds { .. })
        ),
        "a count past the input is refused, never speculatively sized"
    );
}

#[test]
fn an_empty_array_and_a_null_array_are_distinct() {
    let mut empty = Vec::new();
    put_compact_array_len(&mut empty, Some(0));
    let mut null = Vec::new();
    put_compact_array_len(&mut null, None);
    assert_eq!(
        read_compact_array_len(&mut Cursor::new(&empty)).expect("decodes"),
        Some(0)
    );
    assert_eq!(
        read_compact_array_len(&mut Cursor::new(&null)).expect("decodes"),
        None
    );
}

#[test]
fn tagged_fields_round_trip_preserving_unknown_tags() {
    let fields = TaggedFields {
        unknown: vec![
            RawTaggedField {
                tag: 0,
                data: vec![1, 2, 3],
            },
            RawTaggedField {
                tag: 7,
                data: vec![],
            },
        ],
    };
    let mut buf = Vec::new();
    put_tagged_fields(&mut buf, &fields);
    let mut cur = Cursor::new(&buf);
    assert_eq!(read_tagged_fields(&mut cur).expect("decodes"), fields);
    assert_eq!(cur.remaining(), 0);
}

#[test]
fn the_empty_tagged_section_is_a_single_zero_byte() {
    let mut buf = Vec::new();
    put_tagged_fields(&mut buf, &TaggedFields::default());
    assert_eq!(buf, [0]);
    let mut cur = Cursor::new(&buf);
    assert_eq!(
        read_tagged_fields(&mut cur).expect("decodes"),
        TaggedFields::default()
    );
}

#[test]
fn a_tagged_field_size_past_the_input_is_rejected() {
    let mut buf = Vec::new();
    put_unsigned_varint(&mut buf, 1); // one field
    put_unsigned_varint(&mut buf, 5); // tag 5
    put_unsigned_varint(&mut buf, 200); // claims 200 bytes
    buf.push(0xAB); // but only one follows
    let mut cur = Cursor::new(&buf);
    assert!(matches!(
        read_tagged_fields(&mut cur),
        Err(DecodeError::LengthOutOfBounds { .. })
    ));
}

/// Golden byte vectors, computed by hand from the KIP-482 spec: a
/// compact length is `uvarint(len + 1)`, null is `uvarint(0)`. These pin
/// the encoding against the specification itself, independent of any
/// other implementation — self-consistent round trips could agree on a
/// wrong encoding, but these cannot. ⚠️ Whole-message byte-compatibility
/// against `kafka-protocol`'s public `Encodable`/`Decodable` (its
/// primitive `Encoder`/`Decoder` are private) is `M2.30`-`M2.34`'s
/// differential, where the message types exist to drive it.
#[test]
fn the_encoding_matches_the_spec_byte_for_byte() {
    let mut buf = Vec::new();
    put_compact_string(&mut buf, "hello");
    assert_eq!(buf, [0x06, b'h', b'e', b'l', b'l', b'o']); // uvarint(6), then 5 bytes

    buf.clear();
    put_compact_nullable_string(&mut buf, Some(""));
    assert_eq!(buf, [0x01]); // uvarint(1) = empty

    buf.clear();
    put_compact_nullable_string(&mut buf, None);
    assert_eq!(buf, [0x00]); // uvarint(0) = null

    buf.clear();
    put_compact_nullable_bytes(&mut buf, Some(&[0x00, 0x01, 0xff]));
    assert_eq!(buf, [0x04, 0x00, 0x01, 0xff]); // uvarint(4), then 3 bytes

    buf.clear();
    put_compact_array_len(&mut buf, Some(3));
    assert_eq!(buf, [0x04]); // uvarint(4) = 3 elements

    buf.clear();
    put_compact_array_len(&mut buf, None);
    assert_eq!(buf, [0x00]); // uvarint(0) = null array
}

/// `M9.3`'s additions: `read_string`/`put_string` and `read_bytes`/
/// `put_bytes`, at both flexible and legacy encodings — `SaslHandshake`'s
/// permanently-non-flexible `mechanism` and `SaslAuthenticate`'s
/// version-gated `auth_bytes` are what needed them.
#[test]
fn non_nullable_string_round_trips_both_encodings() {
    for flexible in [false, true] {
        let mut buf = Vec::new();
        put_string(&mut buf, flexible, "PLAIN");
        let mut cur = Cursor::new(&buf);
        assert_eq!(
            read_string(&mut cur, flexible).expect("decodes"),
            "PLAIN",
            "flexible={flexible}"
        );
        assert_eq!(cur.remaining(), 0);
    }
}

#[test]
fn non_nullable_string_rejects_the_null_encoding_both_ways() {
    for flexible in [false, true] {
        let mut buf = Vec::new();
        put_nullable_string(&mut buf, flexible, None);
        let mut cur = Cursor::new(&buf);
        assert!(
            matches!(
                read_string(&mut cur, flexible),
                Err(DecodeError::UnexpectedNull { .. })
            ),
            "flexible={flexible}"
        );
    }
}

#[test]
fn non_nullable_bytes_round_trip_both_encodings() {
    for flexible in [false, true] {
        let value = b"\x00\x01\xff";
        let mut buf = Vec::new();
        put_bytes(&mut buf, flexible, value);
        let mut cur = Cursor::new(&buf);
        assert_eq!(
            read_bytes(&mut cur, flexible).expect("decodes"),
            &value[..],
            "flexible={flexible}"
        );
        assert_eq!(cur.remaining(), 0);
    }
}

#[test]
fn non_nullable_bytes_round_trip_the_empty_case() {
    for flexible in [false, true] {
        let mut buf = Vec::new();
        put_bytes(&mut buf, flexible, b"");
        let mut cur = Cursor::new(&buf);
        assert_eq!(
            read_bytes(&mut cur, flexible).expect("decodes"),
            &b""[..],
            "flexible={flexible}"
        );
    }
}

#[test]
fn non_nullable_bytes_rejects_the_null_compact_encoding() {
    let mut buf = Vec::new();
    put_compact_nullable_bytes(&mut buf, None);
    let mut cur = Cursor::new(&buf);
    assert!(matches!(
        read_bytes(&mut cur, true),
        Err(DecodeError::UnexpectedNull { .. })
    ));
}
