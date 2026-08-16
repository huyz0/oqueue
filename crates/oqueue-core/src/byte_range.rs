//! A byte range within an object.

use crate::{Error, Result};

/// A bounded byte range, `[offset, offset + length)`.
///
/// ⚠️ **Its fields are private, deliberately, on top of the enum they live
/// in.** Rust gives a `pub enum`'s struct-variant fields the same visibility
/// as the enum itself — there is no way to write `pub enum { Bounded {
/// offset: u64, length: u64 } }` and have `length` stay private. Wrapping the
/// fields in this ordinary struct, whose fields *are* private, is what makes
/// [`ByteRange::bounded`] the only way to build one — the same
/// "unconstructible around" pattern [`crate::ObjectKey`] and every other
/// identifier in this crate already uses, achieved here through an extra
/// layer because the enum-variant route does not offer it directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounded {
    offset: u64,
    length: u64,
}

impl Bounded {
    /// The range's start, zero-indexed.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// The range's length. Never zero — see [`ByteRange::bounded`].
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }
}

/// The span of an object a [`crate::ObjectStore::get`] call reads.
///
/// ⚠️ **Not a general-purpose range type.** It exists to make "the whole
/// object" and "a bounded slice of it" the only two shapes a caller can ask
/// for — there is no way to construct an open-ended `offset..` range, because
/// nothing in this workspace needs one and a backend answering it would have
/// to know the object's size first anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// The whole object, whatever its size.
    Full,
    /// A bounded slice. Build with [`ByteRange::bounded`].
    Bounded(Bounded),
}

impl ByteRange {
    /// A bounded range `[offset, offset + length)`.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyByteRange`] if `length` is zero. A zero-length range
    /// reads nothing, which is never what a caller means to ask for — an
    /// empty result and a mistaken range are otherwise indistinguishable.
    pub const fn bounded(offset: u64, length: u64) -> Result<Self> {
        if length == 0 {
            return Err(Error::EmptyByteRange);
        }
        Ok(Self::Bounded(Bounded { offset, length }))
    }
}
