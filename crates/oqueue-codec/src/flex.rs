//! Kafka's flexible-versions primitives: compact strings, bytes, arrays, and
//! the tagged-fields section (KIP-482).
//!
//! ⚠️ **`ADR-0019` moved these here.** `ADR-0017` had left message-body
//! primitives to `kafka-protocol`; `ADR-0019` reverses that, and the reason
//! is exactly what this module does differently from a general decoder:
//! **every count and length is bounded against the bytes that remain before
//! anything is allocated** (`security.md` rules 1-2). A compact array of `n`
//! elements needs at least `n` bytes on the wire — one per element, even an
//! empty flexible struct carries its tagged-fields byte — so a claimed count
//! past `remaining()` is a lie the decoder rejects rather than a `Vec` it
//! speculatively sizes. That single discipline is what dissolves the
//! `kafka-protocol` allocation `DoS` `M2.26` found.
//!
//! ## The encoding, precisely
//!
//! A compact length is an unsigned varint holding `actual_length + 1`; the
//! value `0` is null. So a compact string of `L` bytes is `uvarint(L + 1)`
//! then `L` bytes; `uvarint(0)` is null. Compact bytes and compact arrays use
//! the same `+1` length. Non-nullable fields reject the null encoding.
//!
//! Strings are UTF-8 on the wire; a reader hands back `&str` borrowed from the
//! cursor (zero-copy, like every other read in this crate).

use crate::varint::{put_unsigned_varint, read_unsigned_varint};
use crate::wire::{Cursor, DecodeError, put_i32, put_legacy_nullable_string};

/// A nullable string, compact when `flexible` else legacy — the version-aware
/// combinator every message field uses, so a codec writes `flexible` once per
/// message rather than branching at each string.
///
/// # Errors
/// As the underlying [`read_compact_nullable_string`] /
/// [`Cursor::read_legacy_nullable_string`].
pub fn read_nullable_string<'a>(
    cur: &mut Cursor<'a>,
    flexible: bool,
) -> Result<Option<&'a str>, DecodeError> {
    if flexible {
        read_compact_nullable_string(cur)
    } else {
        cur.read_legacy_nullable_string()
    }
}

/// A string a schema declares **non-nullable**, compact when `flexible`
/// else legacy — [`put_string`]'s read-side counterpart.
///
/// `M9.3`, the same gap `put_string`'s own doc names: a field the schema
/// cannot express as absent should not decode to one that silently is.
///
/// # Errors
/// [`DecodeError::UnexpectedNull`] if the wire encodes null; otherwise as
/// [`read_nullable_string`].
pub fn read_string<'a>(cur: &mut Cursor<'a>, flexible: bool) -> Result<&'a str, DecodeError> {
    let at = cur.position();
    read_nullable_string(cur, flexible)?.ok_or(DecodeError::UnexpectedNull { at })
}

/// Bytes a schema declares **non-nullable**, compact when `flexible` else
/// legacy (`i32`-length-prefixed). [`put_bytes`]'s read-side counterpart.
///
/// ⚠️ **Bounded by what remains, not a magic constant** — `compress.rs`'s
/// own precedent for a length nothing else caps: [`Cursor::take`] inside
/// [`Cursor::read_length_prefixed`] refuses a length past the buffer
/// regardless, so `remaining()` is a real bound, not a formality.
///
/// # Errors
/// [`DecodeError::UnexpectedNull`] if the compact encoding is null;
/// otherwise the legacy path's [`DecodeError::NegativeLength`] /
/// [`DecodeError::LengthOutOfBounds`] / [`DecodeError::UnexpectedEof`].
pub fn read_bytes<'a>(cur: &mut Cursor<'a>, flexible: bool) -> Result<&'a [u8], DecodeError> {
    if flexible {
        let at = cur.position();
        read_compact_nullable_bytes(cur)?.ok_or(DecodeError::UnexpectedNull { at })
    } else {
        let max = u64::try_from(cur.remaining()).unwrap_or(u64::MAX);
        cur.read_length_prefixed(max)
    }
}

/// Appends bytes a schema declares **non-nullable**, compact when
/// `flexible` else legacy.
///
/// [`put_string`]'s own precedent: a `&[u8]` rather than an `Option`, so
/// the writer cannot express a null a non-nullable field must not carry.
pub fn put_bytes(buf: &mut Vec<u8>, flexible: bool, value: &[u8]) {
    if flexible {
        put_compact_nullable_bytes(buf, Some(value));
    } else {
        put_i32(buf, i32::try_from(value.len()).unwrap_or(i32::MAX));
        buf.extend_from_slice(value);
    }
}

/// Appends a string a schema declares **non-nullable**, compact when
/// `flexible` else legacy.
///
/// ⚠️ **It exists so the two cases have different names** (`M3.40`). Three
/// handlers in a row echoed a request's *nullable* topic name into a response
/// field the schema declares non-nullable, and each time the symptom was a
/// reply no client can parse — the Java client throws in its response parser,
/// librdkafka reports a protocol read error. `put_nullable_string` is the
/// right writer for a field that may be null and the wrong one for a field
/// that may not, and nothing in a call to it said which kind it was writing.
///
/// ⚠️ **A `&str` rather than an `Option`**, so the *writer* cannot express a
/// null. ⚠️ **That is not the same as the caller having to decide**, which is
/// what this claimed until `M3.37`: all three call sites resolve their
/// `Option` with `unwrap_or_default()` one line above, so a fourth handler
/// gets a topic named `""` — readable, unmatchable, and quieter than the
/// unparsable frame it replaces. `M3.41` is the row that makes the invalid
/// state unrepresentable rather than merely inexpressible at the last step.
pub fn put_string(buf: &mut Vec<u8>, flexible: bool, value: &str) {
    if flexible {
        put_compact_nullable_string(buf, Some(value));
    } else {
        put_legacy_nullable_string(buf, Some(value));
    }
}

/// Appends a nullable string, compact when `flexible` else legacy.
pub fn put_nullable_string(buf: &mut Vec<u8>, flexible: bool, value: Option<&str>) {
    if flexible {
        put_compact_nullable_string(buf, value);
    } else {
        put_legacy_nullable_string(buf, value);
    }
}

/// A nullable array length, compact when `flexible` else legacy.
///
/// The caller decodes that many elements; `None` is the null array. Bounded
/// against the remaining input either way (`security.md` rules 1-2).
///
/// # Errors
/// As [`read_compact_array_len`] / [`Cursor::read_legacy_array_len`].
pub fn read_array_len(cur: &mut Cursor<'_>, flexible: bool) -> Result<Option<usize>, DecodeError> {
    if flexible {
        read_compact_array_len(cur)
    } else {
        cur.read_legacy_array_len()
    }
}

/// Appends a nullable array length, compact when `flexible` else legacy.
pub fn put_array_len(buf: &mut Vec<u8>, flexible: bool, count: Option<usize>) {
    if flexible {
        put_compact_array_len(buf, count);
    } else {
        match count {
            None => put_i32(buf, -1),
            Some(n) => put_i32(buf, i32::try_from(n).unwrap_or(i32::MAX)),
        }
    }
}

/// A tagged field the decoder did not recognise, kept verbatim so a
/// re-encode reproduces the peer's bytes (KIP-482 requires unknown tags to
/// survive a round trip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawTaggedField {
    /// The tag number.
    pub tag: u32,
    /// The tag's opaque contents, exactly as received.
    pub data: Vec<u8>,
}

/// The decoded tagged-fields section: every field, in wire order.
///
/// This crate recognises no tags today, so all of them land in `unknown` —
/// but they are kept, not discarded, because dropping an unknown tag on
/// re-encode is a silent protocol violation a client may notice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaggedFields {
    /// Fields whose tag this decoder does not recognise, in wire order.
    pub unknown: Vec<RawTaggedField>,
}

/// Reads a compact length (`uvarint(len + 1)`), returning the length or
/// `None` for the null encoding (`0`), bounded against `remaining`.
///
/// ⚠️ The bound is the heart of the module: `len` may not exceed the bytes
/// left in the cursor, because no valid field of `len` units can. A caller
/// sizing a collection from this value is therefore safe by construction.
///
/// # Errors
/// The varint's own errors, or [`DecodeError::LengthOutOfBounds`] when the
/// decoded length exceeds `remaining()`.
fn read_compact_len(cur: &mut Cursor<'_>) -> Result<Option<usize>, DecodeError> {
    let at = cur.position();
    let raw = read_unsigned_varint(cur)?;
    if raw == 0 {
        return Ok(None);
    }
    let len = (raw - 1) as usize;
    // Bound before the caller allocates or slices: a length past what
    // remains cannot be honest (each unit is at least one byte).
    if len > cur.remaining() {
        return Err(DecodeError::LengthOutOfBounds {
            length: u64::from(raw - 1),
            max: cur.remaining() as u64,
            at,
        });
    }
    Ok(Some(len))
}

/// A nullable compact string, zero-copy. `None` is the null encoding.
///
/// # Errors
/// [`DecodeError`] on a malformed length, a length past the input, or
/// [`DecodeError::InvalidUtf8`] when the bytes are not valid UTF-8.
pub fn read_compact_nullable_string<'a>(
    cur: &mut Cursor<'a>,
) -> Result<Option<&'a str>, DecodeError> {
    let Some(len) = read_compact_len(cur)? else {
        return Ok(None);
    };
    let at = cur.position();
    let bytes = cur.take(len)?;
    core::str::from_utf8(bytes)
        .map(Some)
        .map_err(|_| DecodeError::InvalidUtf8 { at })
}

/// A non-nullable compact string. The null encoding is a decode error.
///
/// # Errors
/// As [`read_compact_nullable_string`], plus [`DecodeError::UnexpectedNull`]
/// when the field is null where null is not legal.
pub fn read_compact_string<'a>(cur: &mut Cursor<'a>) -> Result<&'a str, DecodeError> {
    let at = cur.position();
    read_compact_nullable_string(cur)?.ok_or(DecodeError::UnexpectedNull { at })
}

/// Nullable compact bytes, zero-copy. `None` is the null encoding.
///
/// # Errors
/// [`DecodeError`] on a malformed length or a length past the input.
pub fn read_compact_nullable_bytes<'a>(
    cur: &mut Cursor<'a>,
) -> Result<Option<&'a [u8]>, DecodeError> {
    read_compact_len(cur)?.map_or(Ok(None), |len| cur.take(len).map(Some))
}

/// Reads a compact array's element count, bounded against `remaining()`, or
/// `None` for the null encoding. The caller decodes that many elements.
///
/// ⚠️ The bound is on the *count*, not on `count * size_of::<Element>()`:
/// `count <= remaining()` because every element is at least one wire byte,
/// so the count is `max_frame`-bounded — but a caller that does
/// `Vec::with_capacity(count)` for a multi-byte element still allocates
/// `count * size_of` transiently. Decode by **growing** the `Vec` in the
/// element loop (or size it against remaining, not the raw count); the
/// bound here makes the loop terminate, it does not license a blind
/// pre-size. This is the discipline `M2.30`-`M2.34` inherit.
///
/// # Errors
/// The varint's errors, or [`DecodeError::LengthOutOfBounds`] past
/// `remaining()`.
pub fn read_compact_array_len(cur: &mut Cursor<'_>) -> Result<Option<usize>, DecodeError> {
    read_compact_len(cur)
}

/// Reads and skips the tagged-fields section, preserving unknown tags.
///
/// Every flexible struct ends with this. The count and each field's size are
/// compact-length-bounded, so a malicious tag count or size cannot force an
/// allocation past the input.
///
/// # Errors
/// [`DecodeError`] on a malformed count, tag, size, or a size past the input.
pub fn read_tagged_fields(cur: &mut Cursor<'_>) -> Result<TaggedFields, DecodeError> {
    // The count is a bare uvarint (not +1-encoded): 0 means no tagged fields.
    let count = read_unsigned_varint(cur)?;
    if count as usize > cur.remaining() {
        return Err(DecodeError::LengthOutOfBounds {
            length: u64::from(count),
            max: cur.remaining() as u64,
            at: cur.position(),
        });
    }
    // ⚠️ Grown, not pre-sized: `count` is bounded by remaining *bytes*, but a
    // `RawTaggedField` is far larger than a byte, so sizing from `count`
    // amplifies a truncated section into a transient over-allocation. The
    // loop consumes at least two bytes per field (tag + size varints), so it
    // runs at most `remaining/2` times regardless — growth is bounded.
    let mut unknown = Vec::new();
    for _ in 0..count {
        let tag = read_unsigned_varint(cur)?;
        let at = cur.position();
        let size = read_unsigned_varint(cur)?;
        if size as usize > cur.remaining() {
            return Err(DecodeError::LengthOutOfBounds {
                length: u64::from(size),
                max: cur.remaining() as u64,
                at,
            });
        }
        let data = cur.take(size as usize)?.to_vec();
        unknown.push(RawTaggedField { tag, data });
    }
    Ok(TaggedFields { unknown })
}

/// Appends a compact length (`uvarint(len + 1)`); `None` writes the null `0`.
///
/// ⚠️ A length is our own response data, never the wire's, so it is bounded
/// by construction — but the encoding of `len + 1` must never wrap to `0`,
/// which is the null sentinel. `checked_add` saturates a pathological length
/// at the largest non-null value rather than silently emitting null; a
/// `debug_assert` catches the bug in tests where an unreachable case is still
/// worth naming.
pub fn put_compact_len(buf: &mut Vec<u8>, len: Option<usize>) {
    match len {
        None => put_unsigned_varint(buf, 0),
        Some(len) => {
            debug_assert!(
                len < u32::MAX as usize,
                "compact length {len} is not representable"
            );
            let encoded = u32::try_from(len)
                .ok()
                .and_then(|n| n.checked_add(1))
                .unwrap_or(u32::MAX);
            put_unsigned_varint(buf, encoded);
        }
    }
}

/// Appends a nullable compact string; `None` writes the null encoding.
pub fn put_compact_nullable_string(buf: &mut Vec<u8>, value: Option<&str>) {
    match value {
        None => put_compact_len(buf, None),
        Some(s) => {
            put_compact_len(buf, Some(s.len()));
            buf.extend_from_slice(s.as_bytes());
        }
    }
}

/// Appends a non-nullable compact string.
pub fn put_compact_string(buf: &mut Vec<u8>, value: &str) {
    put_compact_nullable_string(buf, Some(value));
}

/// Appends nullable compact bytes; `None` writes the null encoding.
pub fn put_compact_nullable_bytes(buf: &mut Vec<u8>, value: Option<&[u8]>) {
    match value {
        None => put_compact_len(buf, None),
        Some(b) => {
            put_compact_len(buf, Some(b.len()));
            buf.extend_from_slice(b);
        }
    }
}

/// Appends a compact array length (`uvarint(count + 1)`); the caller writes
/// the elements. `None` writes the null encoding.
pub fn put_compact_array_len(buf: &mut Vec<u8>, count: Option<usize>) {
    put_compact_len(buf, count);
}

/// Appends the tagged-fields section, writing back any preserved unknown
/// tags in wire order.
pub fn put_tagged_fields(buf: &mut Vec<u8>, fields: &TaggedFields) {
    put_unsigned_varint(buf, u32::try_from(fields.unknown.len()).unwrap_or(u32::MAX));
    for field in &fields.unknown {
        put_unsigned_varint(buf, field.tag);
        put_unsigned_varint(buf, u32::try_from(field.data.len()).unwrap_or(u32::MAX));
        buf.extend_from_slice(&field.data);
    }
}

#[cfg(test)]
mod tests;
