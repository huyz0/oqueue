//! The `RecordBatch` attributes bitfield and the codec it names.
//!
//! Split from `batch.rs` by concept when both crossed the 500-line rule:
//! the bitfield is protocol vocabulary shared by the header layer and the
//! compression seam, and neither owns it.

/// The compression codec named by attributes bits 0-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Bits 000.
    None,
    /// Bits 001.
    Gzip,
    /// Bits 010.
    Snappy,
    /// Bits 011.
    Lz4,
    /// Bits 100.
    Zstd,
    /// Bits 101-111: reserved by the protocol today. Carried, not refused —
    /// the decision of what to do with an unknown codec belongs to the
    /// compression seam (`M2.20`), not to a bitfield accessor.
    Unknown(u8),
}

/// The `attributes` bitfield, kept as its wire value with typed accessors —
/// so a round trip is the identity even for bits this version of the
/// protocol has not assigned yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attributes(pub i16);

impl Attributes {
    /// Bits 0-2.
    #[must_use]
    pub fn compression(self) -> Compression {
        match self.0 & 0b111 {
            0 => Compression::None,
            1 => Compression::Gzip,
            2 => Compression::Snappy,
            3 => Compression::Lz4,
            4 => Compression::Zstd,
            other => Compression::Unknown(u8::try_from(other).unwrap_or(u8::MAX)),
        }
    }

    /// Bit 3: `LogAppendTime` when set, `CreateTime` clear.
    #[must_use]
    pub const fn is_log_append_time(self) -> bool {
        self.0 & (1 << 3) != 0
    }

    /// Bit 4.
    #[must_use]
    pub const fn is_transactional(self) -> bool {
        self.0 & (1 << 4) != 0
    }

    /// Bit 5.
    #[must_use]
    pub const fn is_control(self) -> bool {
        self.0 & (1 << 5) != 0
    }

    /// Bit 6.
    #[must_use]
    pub const fn has_delete_horizon(self) -> bool {
        self.0 & (1 << 6) != 0
    }

    /// This value with bits 0-2 replaced by `codec`'s encoding.
    #[must_use]
    pub fn with_compression(self, codec: Compression) -> Self {
        let bits: i16 = match codec {
            Compression::None => 0,
            Compression::Gzip => 1,
            Compression::Snappy => 2,
            Compression::Lz4 => 3,
            Compression::Zstd => 4,
            Compression::Unknown(other) => i16::from(other) & 0b111,
        };
        Self((self.0 & !0b111) | bits)
    }
}

#[cfg(test)]
mod tests {
    use super::{Attributes, Compression};

    #[test]
    fn every_attribute_bit_round_trips_and_reads_back() {
        for codec in [
            Compression::None,
            Compression::Gzip,
            Compression::Snappy,
            Compression::Lz4,
            Compression::Zstd,
        ] {
            let a = Attributes(0).with_compression(codec);
            assert_eq!(a.compression(), codec);
        }
        let a = Attributes(0b111_1000).with_compression(Compression::Lz4);
        assert!(a.is_log_append_time());
        assert!(a.is_transactional());
        assert!(a.is_control());
        assert!(a.has_delete_horizon());
        assert_eq!(a.compression(), Compression::Lz4);
        // An unassigned codec value is carried, not lost.
        assert_eq!(Attributes(0b101).compression(), Compression::Unknown(5));
    }
}
