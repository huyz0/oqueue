//! The length-delimited frame, and the headers either side of a body.
//!
//! A Kafka frame is a big-endian `i32` size then that many body bytes.
//! Reading one is [`crate::wire::Cursor::read_length_prefixed`] with the
//! connection's frame cap; what this module adds is the *header* handling
//! around a body — the prelude sniff a dispatcher needs before it knows the
//! message type, the full header decode through `kafka-protocol`, and
//! response-header encoding.
//!
//! ⚠️ **The `ApiVersions` special case lives here and nowhere else.** Its
//! *response* header is v0 — bare correlation id, no tagged fields — even at
//! v3, where the body and the *request* header are flexible. Emit a v1
//! header there and every client fails at the first byte after connect
//! (doc 02 §7.6). Nothing in this module hand-computes header versions:
//! both directions go through the dependency's generated
//! `request_header_version`/`response_header_version`, whose generated
//! `ApiVersionsResponse::header_version` is unconditionally 0 — and a golden
//! byte test below pins that at the byte level, so a dependency bump that
//! loses the special case fails here rather than against librdkafka.

use crate::wire::{Cursor, DecodeError};
use kafka_protocol::messages::{ApiKey, RequestHeader, ResponseHeader};
use kafka_protocol::protocol::{Decodable, Encodable};

/// The three fixed fields every request starts with, whatever its header
/// version — what a dispatcher reads before it knows the message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPrelude {
    /// The API key, raw — [`ApiKey::try_from`] may still refuse it, and an
    /// unknown key is the caller's `UNSUPPORTED_VERSION` cue, not a decode
    /// failure.
    pub api_key: i16,
    /// The API version the client asked for.
    pub api_version: i16,
    /// Echoed verbatim in the response header.
    pub correlation_id: i32,
}

/// Why a header could not be handled.
///
/// ⚠️ No `anyhow` in this contract (`error-handling.md` rule 4): the
/// dependency's errors cross this boundary as the plain trait object their
/// source chain survives in, and the one case a dispatcher must tell apart —
/// a well-formed prelude naming an API nobody serves — is its own variant,
/// because "answer `UNSUPPORTED_VERSION`" and "close the connection" are
/// different responses to different failures.
#[derive(Debug)]
pub enum FrameError {
    /// The bytes themselves ran out or violated a bound.
    Decode(DecodeError),
    /// A well-formed prelude naming an API key the dependency does not
    /// know — the caller's `UNSUPPORTED_VERSION` cue, never a reason to
    /// close.
    UnknownApiKey {
        /// The key as sent.
        api_key: i16,
    },
    /// `kafka-protocol` refused the header bytes.
    Header(Box<dyn std::error::Error + Send + Sync>),
    /// A frame body larger than `i32::MAX` cannot be length-prefixed.
    BodyTooLarge {
        /// The unencodable size.
        length: usize,
    },
}

impl From<DecodeError> for FrameError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "{e}"),
            Self::UnknownApiKey { api_key } => {
                write!(f, "api key {api_key} is not one this broker knows")
            }
            Self::Header(e) => write!(f, "header refused: {e}"),
            Self::BodyTooLarge { length } => {
                write!(f, "a {length}-byte body cannot carry an i32 length prefix")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// Reads the fixed prelude without consuming past it.
///
/// # Errors
/// [`DecodeError::UnexpectedEof`] if the body cannot hold even the prelude.
pub fn read_request_prelude(body: &[u8]) -> Result<RequestPrelude, DecodeError> {
    let mut cur = Cursor::new(body);
    Ok(RequestPrelude {
        api_key: cur.read_i16()?,
        api_version: cur.read_i16()?,
        correlation_id: cur.read_i32()?,
    })
}

/// Decodes the full request header, returning it with the byte offset the
/// message body starts at.
///
/// The header version is the dependency's
/// `request_header_version(api_version)` for the key — per-API, generated
/// from Kafka's own specs, never computed here.
///
/// # Errors
/// [`FrameError::Decode`] for an unparsable prelude,
/// [`FrameError::UnknownApiKey`] for a key the dependency does not know
/// (answer `UNSUPPORTED_VERSION`, do not close), and [`FrameError::Header`]
/// when the header bytes themselves refuse.
pub fn decode_request_header(body: &[u8]) -> Result<(RequestHeader, usize), FrameError> {
    let prelude = read_request_prelude(body)?;
    let api_key = ApiKey::try_from(prelude.api_key).map_err(|()| FrameError::UnknownApiKey {
        api_key: prelude.api_key,
    })?;
    let header_version = api_key.request_header_version(prelude.api_version);
    let mut buf = body;
    let before = buf.len();
    let header = RequestHeader::decode(&mut buf, header_version)
        .map_err(|e| FrameError::Header(e.into()))?;
    Ok((header, before - buf.len()))
}

/// Appends the response header for `api_key` at `api_version`, correlation
/// id echoed — through the dependency's `response_header_version`, which is
/// where the `ApiVersions` v0 special case lives.
///
/// # Errors
/// [`FrameError::Header`] if the dependency refuses to encode — unreachable
/// for a header this simple, kept a `Result` so no panic path enters the
/// connection task.
pub fn encode_response_header(
    out: &mut Vec<u8>,
    api_key: ApiKey,
    api_version: i16,
    correlation_id: i32,
) -> Result<(), FrameError> {
    let header_version = api_key.response_header_version(api_version);
    let mut header = ResponseHeader::default();
    header.correlation_id = correlation_id;
    // Straight into the caller's buffer — `ByteBufMut` is implemented for
    // `Vec<u8>`, so no intermediate allocation on the response path.
    header
        .encode(out, header_version)
        .map_err(|e| FrameError::Header(e.into()))?;
    Ok(())
}

/// Appends `body` as one frame: big-endian `i32` size, then the bytes.
///
/// # Errors
/// [`FrameError::BodyTooLarge`] past `i32::MAX` — a state the broker's own
/// caps should make unreachable, refused here so it cannot wrap silently.
pub fn write_frame(out: &mut Vec<u8>, body: &[u8]) -> Result<(), FrameError> {
    let Ok(length) = i32::try_from(body.len()) else {
        return Err(FrameError::BodyTooLarge { length: body.len() });
    };
    crate::wire::put_i32(out, length);
    out.extend_from_slice(body);
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        FrameError, decode_request_header, encode_response_header, read_request_prelude,
        write_frame,
    };
    use crate::wire::{Cursor, DecodeError};
    use kafka_protocol::messages::ApiKey;

    #[test]
    fn a_frame_round_trips_through_the_cursor() {
        let mut out = Vec::new();
        write_frame(&mut out, b"abc").expect("three bytes fit an i32");
        assert_eq!(out, [0, 0, 0, 3, b'a', b'b', b'c']);
        let mut cur = Cursor::new(&out);
        assert_eq!(cur.read_length_prefixed(1024), Ok(&b"abc"[..]));
    }

    #[test]
    fn a_frame_over_the_connection_cap_is_refused_by_the_bound() {
        let mut out = Vec::new();
        write_frame(&mut out, &[0u8; 64]).expect("fits");
        let mut cur = Cursor::new(&out);
        assert_eq!(
            cur.read_length_prefixed(63),
            Err(DecodeError::LengthOutOfBounds {
                length: 64,
                max: 63,
                at: 0,
            })
        );
    }

    /// A hand-written librdkafka-shaped `ApiVersions` v3 request: flexible
    /// (v2) request header. ⚠️ **`client_id` stays a legacy `i16`-length
    /// nullable string even in the flexible header** — Kafka froze the field's
    /// encoding for compatibility, so v2 adds only the tagged-fields byte.
    /// A compact-string spelling here fails to decode, which this test
    /// originally proved by accident.
    #[test]
    fn a_flexible_request_header_decodes_with_the_body_offset() {
        let body: &[u8] = &[
            0x00, 0x12, // api_key 18 (ApiVersions)
            0x00, 0x03, // api_version 3
            0x00, 0x00, 0x00, 0x07, // correlation_id 7
            0x00, 0x07, b'r', b'd', b'k', b'a', b'f', b'k',
            b'a', // "rdkafka", legacy i16 length
            0x00, // empty tagged fields
            0xAA, 0xBB, // first two body bytes
        ];
        let prelude = read_request_prelude(body).expect("prelude fits");
        assert_eq!(prelude.api_key, 18);
        assert_eq!(prelude.api_version, 3);
        assert_eq!(prelude.correlation_id, 7);

        let (header, consumed) = decode_request_header(body).expect("a well-formed v2 header");
        assert_eq!(header.correlation_id, 7);
        assert_eq!(header.client_id.as_deref(), Some("rdkafka"));
        assert_eq!(consumed, body.len() - 2, "the body starts after the header");
    }

    /// The same fields in a v1 (non-flexible) header — a Produce v3 shape:
    /// `i16`-length client id, no tagged fields.
    #[test]
    fn a_non_flexible_request_header_decodes_too() {
        let body: &[u8] = &[
            0x00, 0x00, // api_key 0 (Produce)
            0x00, 0x03, // api_version 3
            0x00, 0x00, 0x00, 0x2A, // correlation_id 42
            0x00, 0x02, b'o', b'q', // client_id "oq"
        ];
        let (header, consumed) = decode_request_header(body).expect("a well-formed v1 header");
        assert_eq!(header.correlation_id, 42);
        assert_eq!(header.client_id.as_deref(), Some("oq"));
        assert_eq!(consumed, body.len());
    }

    #[test]
    fn an_unknown_api_key_is_its_own_variant_not_a_decode_failure() {
        let body: &[u8] = &[
            0x7F, 0x00, // api_key 32512: nothing
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        ];
        assert!(matches!(
            decode_request_header(body),
            Err(FrameError::UnknownApiKey { api_key: 32512 })
        ));
    }

    /// ⚠️ The golden byte pin for the special case: an `ApiVersions` v3
    /// response header is exactly four bytes — the correlation id, v0, no
    /// tagged fields — even though v3's body and request header are
    /// flexible. This failing is every client failing at the first byte
    /// after connect.
    #[test]
    fn apiversions_response_header_stays_v0_at_the_byte_level() {
        let mut out = Vec::new();
        encode_response_header(&mut out, ApiKey::ApiVersions, 3, 0x0102_0304)
            .expect("a header encodes");
        assert_eq!(out, [0x01, 0x02, 0x03, 0x04]);
    }

    /// And the contrast that proves the special case is special: a Produce
    /// v9 response header is flexible v1 — correlation id plus the empty
    /// tagged-fields byte.
    #[test]
    fn a_flexible_response_header_carries_tagged_fields() {
        let mut out = Vec::new();
        encode_response_header(&mut out, ApiKey::Produce, 9, 0x0102_0304)
            .expect("a header encodes");
        assert_eq!(out, [0x01, 0x02, 0x03, 0x04, 0x00]);
        // And below the cutover, v0 again.
        let mut out = Vec::new();
        encode_response_header(&mut out, ApiKey::Produce, 8, 5).expect("a header encodes");
        assert_eq!(out, [0, 0, 0, 5]);
    }
}
