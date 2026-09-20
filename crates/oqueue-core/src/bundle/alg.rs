//! How a region's bytes are protected: the algorithm code the footer carries.
//!
//! ⚠️ **Its own module since `M8.4`**, when the envelope pushed `bundle.rs`
//! past the 500-line limit. The type is unchanged by that move, and it is the
//! one field in the region header a reader dispatches on (FR-43).

use crate::{Error, Result};

/// How a region's bytes are protected.
///
/// ⚠️ **Doc 10 #40: the region header names its algorithm from its first
/// commit**, which is this one — `M1.7` found `M1` had no object format to put
/// the field on, so `roadmap.md`'s deferral table carried it here, to the first
/// commit that defines a bundled object's internal structure. A few bytes now
/// against a migration later.
///
/// ⚠️ **`M3` writes [`None`](RegionAlg::None) and reads nothing else.** `M8` is
/// where a decoder acts on another value; what matters today is that the field
/// exists, so an object written now can be told apart from one written then
/// without guessing.
///
/// ⚠️ **Per region, not per object.** `architecture.md`'s Encryption section
/// bundles topics sharing one KEK into one object, and a region's algorithm is
/// a property of the topic whose records it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RegionAlg {
    /// Stored as written. The default path, and all `M3` produces.
    None = 0,
    /// AES-256-GCM, sealed by `oqueue-crypto` under the topic's data
    /// encryption key (`M8.3`, `ADR-0050` point 1).
    ///
    /// ⚠️ **The code is durable the moment one object carries it**, so this
    /// discriminant is part of the wire format rather than an implementation
    /// detail — a later build that renumbered it would read every region
    /// written before it as something else. ⚠️ **`M13`'s FIPS build swaps the
    /// *implementation* of this value, never the value** (`ADR-0050` point 7,
    /// `ADR-0012`): the whole reason the header names an algorithm is that a
    /// FIPS and a non-FIPS broker must read each other's data.
    Aes256Gcm = 1,
}

impl RegionAlg {
    /// The byte a footer carries.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Reads one back.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownRegionAlg`] for any value this build does not know.
    /// ⚠️ **An error, never a default.** Treating an unknown algorithm as
    /// "stored as written" would hand a decoder ciphertext and let it decode
    /// whatever that happened to look like.
    pub const fn from_code(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::None),
            1 => Ok(Self::Aes256Gcm),
            other => Err(Error::UnknownRegionAlg { code: other }),
        }
    }
}
