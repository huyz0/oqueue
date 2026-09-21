//! The byte writers: append big-endian fixed-width values and legacy strings
//! to a `Vec`.
//!
//! Split from [`crate::wire`] (which holds the reader side, the `Cursor`) only
//! to keep each file under the size limit — callers reach these through
//! `crate::wire`'s re-export, so the split is invisible.
//!
//! A `Vec` is the right buffer because every frame this crate assembles is
//! written once and handed to the socket task whole.

/// Appends `v` big-endian. The writers mirror the readers; a `Vec` is the
/// right buffer because every frame this crate assembles is written once and
/// handed to the socket task whole.
pub fn put_i8(buf: &mut Vec<u8>, v: i8) {
    buf.push(v.cast_unsigned());
}

/// Appends `v` big-endian.
pub fn put_i16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Appends `v` big-endian.
pub fn put_i32(buf: &mut Vec<u8>, v: i32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Appends `v` big-endian.
pub fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Appends `v` big-endian.
pub fn put_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

/// Appends an IEEE-754 binary64 in Kafka's big-endian wire order.
pub fn put_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_bits().to_be_bytes());
}

/// Appends a bool as one byte (`1` true, `0` false).
pub fn put_bool(buf: &mut Vec<u8>, v: bool) {
    buf.push(u8::from(v));
}

/// Appends a legacy (non-flexible) nullable string: an `i16` length, `-1`
/// for `None`, then the UTF-8 bytes.
pub fn put_legacy_nullable_string(buf: &mut Vec<u8>, value: Option<&str>) {
    match value {
        None => put_i16(buf, -1),
        Some(s) => {
            put_i16(buf, i16::try_from(s.len()).unwrap_or(i16::MAX));
            buf.extend_from_slice(s.as_bytes());
        }
    }
}
