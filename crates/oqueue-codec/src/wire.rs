//! Byte-level primitives for the frame codec and the `RecordBatch` v2 path.
//!
//! Those are the two layers this crate hand-rolls (`ADR-0017`) —
//! message-*body* primitives, flexible encodings included, are
//! `kafka-protocol`'s job, and nothing here duplicates them.
//!
//! Kafka's wire format is big-endian throughout. Decoding is zero-copy — a
//! [`Cursor`] hands out slices of the caller's buffer — and every
//! client-supplied length is checked against both the remaining input and a
//! caller-stated bound **before** any allocation or slice (`security.md`
//! rules 1-2: bound, validate, then touch).
//!
//! ⚠️ **No string primitives, deliberately.** The frame carries sizes and a
//! correlation id; a `RecordBatch` carries fixed-width fields and opaque
//! records. Every string on this wire lives in a message body, which is the
//! dependency's layer — a string reader here would be unearned code waiting
//! to drift from the one that actually runs.

/// Why a decode stopped. Carries what a debugger needs: how much was asked
/// for, how much existed, where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The input ended before the value did.
    UnexpectedEof {
        /// Bytes the read needed.
        needed: usize,
        /// Bytes that remained.
        remaining: usize,
        /// Byte offset the read started at.
        at: usize,
    },
    /// A length field exceeds the bound the caller stated for it.
    ///
    /// ⚠️ The bound is the caller's claim about what is sane (a frame cap, a
    /// batch cap), not the buffer's size — `UnexpectedEof` covers running
    /// out of bytes. Separating them is what lets an operator tell "peer
    /// sent a 2 GiB frame" from "the read was truncated".
    LengthOutOfBounds {
        /// The length the peer claimed.
        length: u64,
        /// The caller's stated maximum.
        max: u64,
        /// Byte offset of the length field.
        at: usize,
    },
    /// A length field is negative where the caller said null is not legal.
    NegativeLength {
        /// The value as sent.
        length: i32,
        /// Byte offset of the length field.
        at: usize,
    },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnexpectedEof {
                needed,
                remaining,
                at,
            } => write!(
                f,
                "input ended: needed {needed} byte(s) at offset {at}, {remaining} remained"
            ),
            Self::LengthOutOfBounds { length, max, at } => write!(
                f,
                "length {length} at offset {at} exceeds the stated bound {max}"
            ),
            Self::NegativeLength { length, at } => {
                write!(
                    f,
                    "negative length {length} at offset {at} where null is not legal"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// A position over a borrowed buffer, from which every read is bounds-checked
/// and zero-copy.
#[derive(Debug, Clone, Copy)]
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// A cursor at the start of `buf`.
    #[must_use]
    pub const fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// The byte offset the next read starts at.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than `n` bytes remain.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::UnexpectedEof {
                needed: n,
                remaining: self.remaining(),
                at: self.pos,
            });
        }
        let slice = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if empty.
    pub fn read_i8(&mut self) -> Result<i8, DecodeError> {
        Ok(self.take(1)?[0].cast_signed())
    }

    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than 2 bytes remain.
    pub fn read_i16(&mut self) -> Result<i16, DecodeError> {
        let b = self.take(2)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than 4 bytes remain.
    pub fn read_i32(&mut self) -> Result<i32, DecodeError> {
        let b = self.take(4)?;
        Ok(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than 4 bytes remain.
    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// # Errors
    /// [`DecodeError::UnexpectedEof`] if fewer than 8 bytes remain.
    pub fn read_i64(&mut self) -> Result<i64, DecodeError> {
        let b = self.take(8)?;
        Ok(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// An `i32`-length-prefixed blob — the shape of a frame body and of
    /// `RecordBatch`'s `records` section — with the length checked against
    /// `max` **before** the slice is taken.
    ///
    /// ⚠️ **On error the cursor's position is unspecified** — unlike
    /// [`Cursor::take`], which consumes nothing on failure, this may sit
    /// past the length field when the body itself is missing. A decoder
    /// abandons the cursor on any error; resuming one is misparsing.
    ///
    /// # Errors
    /// [`DecodeError::NegativeLength`] on a negative length (null is not a
    /// blob; callers with a nullable field use
    /// [`Cursor::read_nullable_length_prefixed`]),
    /// [`DecodeError::LengthOutOfBounds`] past `max`, and
    /// [`DecodeError::UnexpectedEof`] if the bytes are not there.
    pub fn read_length_prefixed(&mut self, max: u64) -> Result<&'a [u8], DecodeError> {
        let at = self.pos;
        let length = self.read_i32()?;
        if length < 0 {
            return Err(DecodeError::NegativeLength { length, at });
        }
        let length_u64 = u64::from(length.unsigned_abs());
        if length_u64 > max {
            return Err(DecodeError::LengthOutOfBounds {
                length: length_u64,
                max,
                at,
            });
        }
        self.take(usize::try_from(length_u64).unwrap_or(usize::MAX))
    }

    /// [`Cursor::read_length_prefixed`], where a `-1` length is a legal null.
    ///
    /// # Errors
    /// As [`Cursor::read_length_prefixed`], except `-1` returns `Ok(None)` —
    /// any other negative length is still [`DecodeError::NegativeLength`].
    pub fn read_nullable_length_prefixed(
        &mut self,
        max: u64,
    ) -> Result<Option<&'a [u8]>, DecodeError> {
        let mut peek = *self;
        if peek.read_i32()? == -1 {
            self.pos = peek.pos;
            return Ok(None);
        }
        // Not advanced past the length field, so `read_length_prefixed`'s
        // error offsets point at it, same as the non-nullable path.
        self.read_length_prefixed(max).map(Some)
    }
}

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

#[cfg(test)]
mod tests {
    use super::{Cursor, DecodeError, put_i8, put_i16, put_i32, put_i64, put_u32};

    #[test]
    fn fixed_width_values_round_trip_big_endian() {
        let mut buf = Vec::new();
        put_i8(&mut buf, -5);
        put_i16(&mut buf, -2);
        put_i32(&mut buf, 0x0102_0304);
        put_u32(&mut buf, 0xDEAD_BEEF);
        put_i64(&mut buf, i64::MIN);
        // Big-endian on the wire, byte-checked so an accidental to_le_bytes
        // cannot round-trip its way past this test.
        assert_eq!(&buf[1..3], &[0xFF, 0xFE]);
        assert_eq!(&buf[3..7], &[0x01, 0x02, 0x03, 0x04]);

        let mut c = Cursor::new(&buf);
        assert_eq!(c.read_i8(), Ok(-5));
        assert_eq!(c.read_i16(), Ok(-2));
        assert_eq!(c.read_i32(), Ok(0x0102_0304));
        assert_eq!(c.read_u32(), Ok(0xDEAD_BEEF));
        assert_eq!(c.read_i64(), Ok(i64::MIN));
        assert_eq!(c.remaining(), 0);
    }

    #[test]
    fn eof_reports_what_was_needed_where() {
        let mut c = Cursor::new(&[0x01, 0x02]);
        assert_eq!(c.read_i8(), Ok(1));
        assert_eq!(
            c.read_i32(),
            Err(DecodeError::UnexpectedEof {
                needed: 4,
                remaining: 1,
                at: 1,
            })
        );
        // A failed read consumes nothing.
        assert_eq!(c.position(), 1);
        assert_eq!(c.read_i8(), Ok(2));
    }

    #[test]
    fn a_length_is_bounded_before_the_slice_is_taken() {
        let mut buf = Vec::new();
        put_i32(&mut buf, 100);
        buf.extend_from_slice(&[0u8; 100]);
        let mut c = Cursor::new(&buf);
        assert_eq!(
            c.read_length_prefixed(99),
            Err(DecodeError::LengthOutOfBounds {
                length: 100,
                max: 99,
                at: 0,
            }),
            "the caller's bound applies even when the bytes exist"
        );
        // And a bound that admits it reads exactly the blob.
        let mut c = Cursor::new(&buf);
        assert_eq!(c.read_length_prefixed(100), Ok(&buf[4..104]));
    }

    #[test]
    fn a_claimed_length_past_the_input_is_eof_not_a_panic() {
        let mut buf = Vec::new();
        put_i32(&mut buf, 50);
        buf.extend_from_slice(&[0u8; 10]);
        let mut c = Cursor::new(&buf);
        assert_eq!(
            c.read_length_prefixed(1024),
            Err(DecodeError::UnexpectedEof {
                needed: 50,
                remaining: 10,
                at: 4,
            })
        );
    }

    #[test]
    fn negative_lengths_are_refused_or_null_by_the_callers_choice() {
        let mut buf = Vec::new();
        put_i32(&mut buf, -1);
        let mut c = Cursor::new(&buf);
        assert_eq!(
            c.read_length_prefixed(10),
            Err(DecodeError::NegativeLength { length: -1, at: 0 })
        );
        let mut c = Cursor::new(&buf);
        assert_eq!(c.read_nullable_length_prefixed(10), Ok(None));
        assert_eq!(c.remaining(), 0, "a null consumes its length field");

        let mut buf = Vec::new();
        put_i32(&mut buf, -2);
        let mut c = Cursor::new(&buf);
        assert_eq!(
            c.read_nullable_length_prefixed(10),
            Err(DecodeError::NegativeLength { length: -2, at: 0 }),
            "only -1 is null; every other negative is malformed"
        );
    }

    proptest::proptest! {
        #[test]
        fn every_fixed_width_value_round_trips(
            a in proptest::prelude::any::<i8>(),
            b in proptest::prelude::any::<i16>(),
            c in proptest::prelude::any::<i32>(),
            d in proptest::prelude::any::<u32>(),
            e in proptest::prelude::any::<i64>(),
        ) {
            let mut buf = Vec::new();
            put_i8(&mut buf, a);
            put_i16(&mut buf, b);
            put_i32(&mut buf, c);
            put_u32(&mut buf, d);
            put_i64(&mut buf, e);
            let mut cur = Cursor::new(&buf);
            proptest::prop_assert_eq!(cur.read_i8(), Ok(a));
            proptest::prop_assert_eq!(cur.read_i16(), Ok(b));
            proptest::prop_assert_eq!(cur.read_i32(), Ok(c));
            proptest::prop_assert_eq!(cur.read_u32(), Ok(d));
            proptest::prop_assert_eq!(cur.read_i64(), Ok(e));
            proptest::prop_assert_eq!(cur.remaining(), 0);
        }

        /// The never-panics half of `security.md` rule 3: arbitrary bytes,
        /// arbitrary bound, no panic ever — and any `Ok` obeys the bound.
        #[test]
        fn arbitrary_input_never_panics_and_ok_obeys_the_bound(
            buf in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..64),
            max in 0u64..32,
        ) {
            let mut cur = Cursor::new(&buf);
            if let Ok(blob) = cur.read_length_prefixed(max) {
                proptest::prop_assert!(blob.len() as u64 <= max);
            }
            let mut cur = Cursor::new(&buf);
            if let Ok(Some(blob)) = cur.read_nullable_length_prefixed(max) {
                proptest::prop_assert!(blob.len() as u64 <= max);
            }
        }
    }

    #[test]
    fn zero_length_blob_is_empty_not_null() {
        let mut buf = Vec::new();
        put_i32(&mut buf, 0);
        let mut c = Cursor::new(&buf);
        assert_eq!(c.read_length_prefixed(10), Ok(&[][..]));
        let mut c = Cursor::new(&buf);
        assert_eq!(c.read_nullable_length_prefixed(10), Ok(Some(&[][..])));
    }
}
