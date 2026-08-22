//! Kafka's varint and zigzag encodings, with a SWAR length scan.
//!
//! Record-level fields inside a `RecordBatch` are base-128 varints
//! (continuation MSB, little-endian groups of 7), signed values zigzag-coded
//! (doc 02 §1.5). ⚠️ **Only the opt-in record iterator decodes these** — the
//! produce hot path rewrites batch headers and decodes zero varints
//! (doc 18 §4.4) — so correctness and bounded input, not throughput, set
//! this module's bar.
//!
//! ⚠️ **Not `varint-simd`** (the row's own warning, from doc 18): it reads
//! up to 16 bytes past the varint, which on untrusted input is a buffer
//! overrun. The fast path here is SWAR — one aligned-free `u64` load the
//! caller's slice provably contains, branch-free continuation scan, then an
//! accumulate over exactly the scanned bytes — and the tail of a buffer
//! falls back to the byte loop. Both paths are pure safe Rust; a property
//! test pins them equal on every value.
//!
//! ## Reference-faithful truncation
//!
//! The terminal byte of a maximal-width varint has more bits than the value
//! has room for. Kafka's own `ByteUtils.readUnsignedVarint` shifts those
//! bits off the end of the integer silently, and this decoder does the
//! same — being stricter than the reference would reject bytes only a
//! non-Kafka encoder can produce while buying nothing: the value decoded is
//! identical wherever both accept. Continuation *past* the maximal width is
//! an error in the reference and here.

use crate::wire::{Cursor, DecodeError};

/// Bytes a `u32` varint may span.
const MAX_VARINT_BYTES: usize = 5;
/// Bytes a `u64` varint may span.
const MAX_VARLONG_BYTES: usize = 10;

/// The number of varint bytes at the start of `word` — the SWAR scan: a
/// byte's MSB is its continuation bit, so the first zero MSB ends the value.
/// Returns 9 when all eight bytes continue.
const fn swar_len(word: u64) -> usize {
    let stops = !word & 0x8080_8080_8080_8080;
    if stops == 0 {
        9
    } else {
        (stops.trailing_zeros() as usize >> 3) + 1
    }
}

/// # Errors
/// [`DecodeError::UnexpectedEof`] if the buffer ends mid-varint;
/// [`DecodeError::VarintTooLong`] if continuation runs past 5 bytes.
pub fn read_unsigned_varint(cur: &mut Cursor<'_>) -> Result<u32, DecodeError> {
    let at = cur.position();
    if let Some(word) = cur.peek_word() {
        let len = swar_len(word);
        if len > MAX_VARINT_BYTES {
            return Err(DecodeError::VarintTooLong {
                max_bytes: MAX_VARINT_BYTES,
                at,
            });
        }
        let bytes = cur.take(len)?;
        let mut value: u32 = 0;
        for (i, &b) in bytes.iter().enumerate() {
            value |= u32::from(b & 0x7F).wrapping_shl(u32::try_from(i * 7).unwrap_or(0));
        }
        return Ok(value);
    }
    read_unsigned_varint_slow(cur, at)
}

/// The byte-loop tail path — and the reference the SWAR path is
/// property-tested against.
fn read_unsigned_varint_slow(cur: &mut Cursor<'_>, at: usize) -> Result<u32, DecodeError> {
    let mut value: u32 = 0;
    for i in 0..MAX_VARINT_BYTES {
        let b = cur.take(1)?[0];
        value |= u32::from(b & 0x7F).wrapping_shl(u32::try_from(i * 7).unwrap_or(0));
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(DecodeError::VarintTooLong {
        max_bytes: MAX_VARINT_BYTES,
        at,
    })
}

/// # Errors
/// As [`read_unsigned_varint`], with a 10-byte maximum.
pub fn read_unsigned_varlong(cur: &mut Cursor<'_>) -> Result<u64, DecodeError> {
    let at = cur.position();
    if let Some(word) = cur.peek_word() {
        let len = swar_len(word);
        if len <= 8 {
            let bytes = cur.take(len)?;
            let mut value: u64 = 0;
            for (i, &b) in bytes.iter().enumerate() {
                value |= u64::from(b & 0x7F).wrapping_shl(u32::try_from(i * 7).unwrap_or(0));
            }
            return Ok(value);
        }
        // Continuation through all eight scanned bytes: bytes 9 and 10
        // arrive by the loop below, which also handles the too-long error.
    }
    read_unsigned_varlong_slow(cur, at)
}

/// The byte-loop path for [`read_unsigned_varlong`].
fn read_unsigned_varlong_slow(cur: &mut Cursor<'_>, at: usize) -> Result<u64, DecodeError> {
    let mut value: u64 = 0;
    for i in 0..MAX_VARLONG_BYTES {
        let b = cur.take(1)?[0];
        value |= u64::from(b & 0x7F).wrapping_shl(u32::try_from(i * 7).unwrap_or(0));
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(DecodeError::VarintTooLong {
        max_bytes: MAX_VARLONG_BYTES,
        at,
    })
}

/// Zigzag-decoded `i32` — Kafka's `VARINT`.
///
/// # Errors
/// As [`read_unsigned_varint`].
pub fn read_varint(cur: &mut Cursor<'_>) -> Result<i32, DecodeError> {
    let raw = read_unsigned_varint(cur)?;
    Ok((raw >> 1).cast_signed() ^ -(raw & 1).cast_signed())
}

/// Zigzag-decoded `i64` — Kafka's `VARLONG`.
///
/// # Errors
/// As [`read_unsigned_varlong`].
pub fn read_varlong(cur: &mut Cursor<'_>) -> Result<i64, DecodeError> {
    let raw = read_unsigned_varlong(cur)?;
    Ok((raw >> 1).cast_signed() ^ -(raw & 1).cast_signed())
}

/// Appends `v` as an unsigned varint.
pub fn put_unsigned_varint(buf: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = u8::try_from(v & 0x7F).unwrap_or(0);
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

/// Appends `v` as an unsigned varlong.
pub fn put_unsigned_varlong(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = u8::try_from(v & 0x7F).unwrap_or(0);
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

/// Appends `v` zigzag-coded — Kafka's `VARINT`.
pub fn put_varint(buf: &mut Vec<u8>, v: i32) {
    put_unsigned_varint(
        buf,
        v.wrapping_shl(1).cast_unsigned() ^ (v >> 31).cast_unsigned(),
    );
}

/// Appends `v` zigzag-coded — Kafka's `VARLONG`.
pub fn put_varlong(buf: &mut Vec<u8>, v: i64) {
    put_unsigned_varlong(
        buf,
        v.wrapping_shl(1).cast_unsigned() ^ (v >> 63).cast_unsigned(),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        put_unsigned_varint, put_unsigned_varlong, put_varint, put_varlong, read_unsigned_varint,
        read_unsigned_varint_slow, read_unsigned_varlong, read_varint, read_varlong,
    };
    use crate::wire::{Cursor, DecodeError};

    #[test]
    fn boundary_vectors_from_the_protocol() {
        // The canonical encodings every Kafka implementation agrees on.
        let cases: &[(u32, &[u8])] = &[
            (0, &[0x00]),
            (1, &[0x01]),
            (127, &[0x7F]),
            (128, &[0x80, 0x01]),
            (300, &[0xAC, 0x02]),
            (16_383, &[0xFF, 0x7F]),
            (16_384, &[0x80, 0x80, 0x01]),
            (u32::MAX, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        ];
        for &(value, bytes) in cases {
            let mut buf = Vec::new();
            put_unsigned_varint(&mut buf, value);
            assert_eq!(buf, bytes, "encoding of {value}");
            let mut cur = Cursor::new(bytes);
            assert_eq!(read_unsigned_varint(&mut cur), Ok(value));
            assert_eq!(cur.remaining(), 0);
        }
    }

    #[test]
    fn zigzag_boundary_vectors() {
        let cases: &[(i32, &[u8])] = &[
            (0, &[0x00]),
            (-1, &[0x01]),
            (1, &[0x02]),
            (-2, &[0x03]),
            (i32::MAX, &[0xFE, 0xFF, 0xFF, 0xFF, 0x0F]),
            (i32::MIN, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
        ];
        for &(value, bytes) in cases {
            let mut buf = Vec::new();
            put_varint(&mut buf, value);
            assert_eq!(buf, bytes, "encoding of {value}");
            let mut cur = Cursor::new(bytes);
            assert_eq!(read_varint(&mut cur), Ok(value));
        }
    }

    #[test]
    fn continuation_past_the_maximum_is_an_error_not_a_wrap() {
        // Six continuing bytes: too long for a u32 varint however decoded.
        let bytes = [0x80, 0x80, 0x80, 0x80, 0x80, 0x01];
        let mut cur = Cursor::new(&bytes);
        assert_eq!(
            read_unsigned_varint(&mut cur),
            Err(DecodeError::VarintTooLong {
                max_bytes: 5,
                at: 0,
            })
        );
        // And a varlong accepts what a varint refuses.
        let mut cur = Cursor::new(&bytes);
        assert_eq!(read_unsigned_varlong(&mut cur), Ok(1 << 35));
    }

    #[test]
    fn a_truncated_varint_is_eof() {
        let bytes = [0x80, 0x80];
        let mut cur = Cursor::new(&bytes);
        assert_eq!(
            read_unsigned_varint(&mut cur),
            Err(DecodeError::UnexpectedEof {
                needed: 1,
                remaining: 0,
                at: 2,
            })
        );
    }

    #[test]
    fn reference_faithful_truncation_of_terminal_bits() {
        // 0xFF in byte five carries three bits a u32 cannot hold; the
        // reference implementation shifts them off silently and so do we.
        let bytes = [0xFF, 0xFF, 0xFF, 0xFF, 0x7F];
        let mut cur = Cursor::new(&bytes);
        assert_eq!(read_unsigned_varint(&mut cur), Ok(u32::MAX));
    }

    proptest::proptest! {
        #[test]
        fn u32_round_trips(v in proptest::prelude::any::<u32>()) {
            let mut buf = Vec::new();
            put_unsigned_varint(&mut buf, v);
            proptest::prop_assert!(buf.len() <= 5);
            let mut cur = Cursor::new(&buf);
            proptest::prop_assert_eq!(read_unsigned_varint(&mut cur), Ok(v));
            proptest::prop_assert_eq!(cur.remaining(), 0);
        }

        #[test]
        fn u64_round_trips(v in proptest::prelude::any::<u64>()) {
            let mut buf = Vec::new();
            put_unsigned_varlong(&mut buf, v);
            proptest::prop_assert!(buf.len() <= 10);
            let mut cur = Cursor::new(&buf);
            proptest::prop_assert_eq!(read_unsigned_varlong(&mut cur), Ok(v));
        }

        #[test]
        fn i32_and_i64_round_trip_zigzag(a in proptest::prelude::any::<i32>(),
                                         b in proptest::prelude::any::<i64>()) {
            let mut buf = Vec::new();
            put_varint(&mut buf, a);
            put_varlong(&mut buf, b);
            let mut cur = Cursor::new(&buf);
            proptest::prop_assert_eq!(read_varint(&mut cur), Ok(a));
            proptest::prop_assert_eq!(read_varlong(&mut cur), Ok(b));
        }

        /// The SWAR fast path and the byte-loop tail path agree on every
        /// value: the same encoding decoded with eight bytes of padding
        /// behind it (fast path reachable) and standalone (tail path at the
        /// buffer's end) yields the same result.
        #[test]
        fn the_swar_and_tail_paths_agree(v in proptest::prelude::any::<u32>()) {
            let mut bare = Vec::new();
            put_unsigned_varint(&mut bare, v);
            let mut padded = bare.clone();
            padded.extend_from_slice(&[0u8; 8]);

            let mut fast = Cursor::new(&padded);
            let mut tail = Cursor::new(&bare);
            let mut slow = Cursor::new(&bare);
            let via_fast = read_unsigned_varint(&mut fast);
            let via_tail = read_unsigned_varint(&mut tail);
            let via_slow = read_unsigned_varint_slow(&mut slow, 0);
            proptest::prop_assert_eq!(via_fast, Ok(v));
            proptest::prop_assert_eq!(via_tail, Ok(v));
            proptest::prop_assert_eq!(via_slow, Ok(v));
        }

        /// Arbitrary bytes never panic, whichever path they land on.
        #[test]
        fn arbitrary_input_never_panics(
            buf in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..24),
        ) {
            let mut cur = Cursor::new(&buf);
            let _ = read_unsigned_varint(&mut cur);
            let mut cur = Cursor::new(&buf);
            let _ = read_unsigned_varlong(&mut cur);
            let mut cur = Cursor::new(&buf);
            let _ = read_varint(&mut cur);
            let mut cur = Cursor::new(&buf);
            let _ = read_varlong(&mut cur);
        }
    }
}
