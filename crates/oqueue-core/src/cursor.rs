//! A bounds-checked reader over bytes an object store returned.
//!
//! ⚠️ **One cursor rather than a check per field.** A parser with a bounds
//! test at every read has as many places to get it wrong as it has fields, and
//! `security.md` rule 3 (nothing reachable from stored bytes may panic) and
//! rule 4 (no unchecked arithmetic) are what govern every one of them.
//!
//! ⚠️ **Shared because there are two durable formats and there will be more.**
//! `composite` and `partition_manifest` parse different bytes with the same
//! discipline, and a second copy of this is a second place for the bounds test
//! to be subtly different. The error each format reports is its own, so the
//! constructor is a parameter rather than a fixed variant.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module, and every item here is exactly that: two format
// parsers need them and nothing outside this crate may. `pub(crate)` is the
// visibility that is true, so the lint that disagrees is the one allowed --
// `bundle.rs` makes the same call for the same reason.
#![allow(clippy::redundant_pub_crate)]

use crate::{Error, Result};

/// Reads big-endian fields, refusing anything that runs past the end.
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    malformed: fn(usize) -> Error,
}

impl<'a> Cursor<'a> {
    /// Opens a cursor that reports `malformed(at)` for anything it cannot read.
    pub(crate) const fn new(bytes: &'a [u8], malformed: fn(usize) -> Error) -> Self {
        Self {
            bytes,
            at: 0,
            malformed,
        }
    }

    /// How far in it has read.
    pub(crate) const fn at(&self) -> usize {
        self.at
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .ok_or_else(|| (self.malformed)(self.at))?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| (self.malformed)(self.at))?;
        self.at = end;
        Ok(slice)
    }

    pub(crate) fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// A signed offset, written as its two's-complement bytes.
    ///
    /// ⚠️ **`cast_signed`, not `as`**: the round trip is exact and deliberate,
    /// and `as` here would be indistinguishable from an accidental narrowing.
    /// A negative result is refused by `Offset::new`, which is where that
    /// invariant lives.
    pub(crate) fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()?.cast_signed())
    }

    pub(crate) fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Reads `len` bytes as UTF-8, reporting the offset the string started at.
    pub(crate) fn string(&mut self, len: usize) -> Result<&'a str> {
        let at = self.at;
        let raw = self.take(len)?;
        core::str::from_utf8(raw).map_err(|_| (self.malformed)(at))
    }
}
