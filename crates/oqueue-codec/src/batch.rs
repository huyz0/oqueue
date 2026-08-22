//! The `RecordBatch` v2 header: the 61-byte fixed layout and its attributes
//! bitfield.
//!
//! This is the layer the produce path lives on (doc 18 §4.4): everything a
//! broker must read or rewrite sits in these fixed-width fields, and the
//! records blob after them stays opaque bytes here. ⚠️ **The CRC's coverage
//! is the layout's whole design**: it runs from `attributes` (byte 21) to
//! the batch's end — `baseOffset`, `batchLength`, and
//! `partitionLeaderEpoch` sit *outside* it, which is precisely what lets
//! offset assignment rewrite a batch without touching a checksum or a
//! varint. `M2.19` builds that rewrite on the constants this module names.
//!
//! Layout, offsets in bytes from the batch start:
//!
//! | field                | offset | width |
//! |----------------------|--------|-------|
//! | baseOffset           | 0      | i64   |
//! | batchLength          | 8      | i32   |
//! | partitionLeaderEpoch | 12     | i32   |
//! | magic                | 16     | i8    |
//! | crc                  | 17     | u32   |
//! | attributes           | 21     | i16   |
//! | lastOffsetDelta      | 23     | i32   |
//! | baseTimestamp        | 27     | i64   |
//! | maxTimestamp         | 35     | i64   |
//! | producerId           | 43     | i64   |
//! | producerEpoch        | 51     | i16   |
//! | baseSequence         | 53     | i32   |
//! | recordCount          | 57     | i32   |
//! | records              | 61     | …     |

use crate::wire::{Cursor, DecodeError, put_i8, put_i16, put_i32, put_i64, put_u32};

/// The fixed header's size — `records` begins here.
pub const BATCH_HEADER_LEN: usize = 61;
/// Where the CRC's coverage begins: the `attributes` field.
pub const CRC_COVERAGE_START: usize = 21;
/// Where the `crc` field itself sits.
pub const CRC_OFFSET: usize = 17;
/// The only magic this broker reads or writes.
pub const MAGIC_V2: i8 = 2;

/// Why a batch header was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchError {
    /// The bytes ran out or a bound was violated.
    Decode(DecodeError),
    /// A magic other than 2 — v0/v1 batches are refused, never
    /// down-converted (KIP-110's own precedent; M2.md's risk list).
    WrongMagic {
        /// The magic byte as sent.
        magic: i8,
    },
}

impl From<DecodeError> for BatchError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

impl core::fmt::Display for BatchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "{e}"),
            Self::WrongMagic { magic } => write!(
                f,
                "record batch magic {magic} is not v2; older formats are refused, not converted"
            ),
        }
    }
}

impl std::error::Error for BatchError {}

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

/// Every fixed field of a v2 batch header, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchHeader {
    /// Assigned by the broker at commit; 0 or -1 from a producer.
    pub base_offset: i64,
    /// Bytes from `partitionLeaderEpoch` to the batch's end — the batch
    /// spans `batch_length + 12` bytes in total.
    pub batch_length: i32,
    /// Outside the CRC, rewritable with `base_offset`.
    pub partition_leader_epoch: i32,
    /// CRC-32C over bytes 21.. — as stored; nothing here verifies it
    /// (`M2.19`'s job, through `oqueue-checksum`).
    pub crc: u32,
    /// The bitfield, typed accessors on [`Attributes`].
    pub attributes: Attributes,
    /// Offset delta of the last record.
    pub last_offset_delta: i32,
    /// First record's timestamp.
    pub base_timestamp: i64,
    /// Largest record timestamp.
    pub max_timestamp: i64,
    /// Idempotent-producer id, -1 when unused.
    pub producer_id: i64,
    /// Idempotent-producer epoch, -1 when unused.
    pub producer_epoch: i16,
    /// First record's sequence, -1 when unused.
    pub base_sequence: i32,
    /// Records in the blob after the header.
    pub record_count: i32,
}

/// Decodes the fixed header from the front of `batch`.
///
/// # Errors
/// [`BatchError::Decode`] when 61 bytes are not there;
/// [`BatchError::WrongMagic`] for anything but v2 — checked before any
/// later field is interpreted, since none of them mean what this struct
/// says in an older format.
pub fn decode_batch_header(batch: &[u8]) -> Result<BatchHeader, BatchError> {
    let mut cur = Cursor::new(batch);
    let base_offset = cur.read_i64()?;
    let batch_length = cur.read_i32()?;
    let partition_leader_epoch = cur.read_i32()?;
    let magic = cur.read_i8()?;
    if magic != MAGIC_V2 {
        return Err(BatchError::WrongMagic { magic });
    }
    Ok(BatchHeader {
        base_offset,
        batch_length,
        partition_leader_epoch,
        crc: cur.read_u32()?,
        attributes: Attributes(cur.read_i16()?),
        last_offset_delta: cur.read_i32()?,
        base_timestamp: cur.read_i64()?,
        max_timestamp: cur.read_i64()?,
        producer_id: cur.read_i64()?,
        producer_epoch: cur.read_i16()?,
        base_sequence: cur.read_i32()?,
        record_count: cur.read_i32()?,
    })
}

/// Appends the 61 fixed bytes of `header`, magic v2, CRC as given.
pub fn encode_batch_header(out: &mut Vec<u8>, header: &BatchHeader) {
    put_i64(out, header.base_offset);
    put_i32(out, header.batch_length);
    put_i32(out, header.partition_leader_epoch);
    put_i8(out, MAGIC_V2);
    put_u32(out, header.crc);
    put_i16(out, header.attributes.0);
    put_i32(out, header.last_offset_delta);
    put_i64(out, header.base_timestamp);
    put_i64(out, header.max_timestamp);
    put_i64(out, header.producer_id);
    put_i16(out, header.producer_epoch);
    put_i32(out, header.base_sequence);
    put_i32(out, header.record_count);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        Attributes, BATCH_HEADER_LEN, BatchError, BatchHeader, Compression, decode_batch_header,
        encode_batch_header,
    };
    use crate::wire::DecodeError;

    fn sample_header() -> BatchHeader {
        BatchHeader {
            base_offset: 42,
            batch_length: 49 + 30,
            partition_leader_epoch: 7,
            crc: 0xDEAD_BEEF,
            attributes: Attributes(0).with_compression(Compression::Zstd),
            last_offset_delta: 2,
            base_timestamp: 1_700_000_000_000,
            max_timestamp: 1_700_000_000_002,
            producer_id: -1,
            producer_epoch: -1,
            base_sequence: -1,
            record_count: 3,
        }
    }

    #[test]
    fn the_header_round_trips_at_exactly_61_bytes() {
        let mut buf = Vec::new();
        encode_batch_header(&mut buf, &sample_header());
        assert_eq!(buf.len(), BATCH_HEADER_LEN);
        assert_eq!(buf[16], 2, "magic v2 at offset 16");
        assert_eq!(decode_batch_header(&buf), Ok(sample_header()));
    }

    /// The golden cross-check: `kafka-protocol`'s own `RecordBatchEncoder`
    /// (generated against Kafka's spec) produces a batch, and this module's
    /// hand-rolled header decode must agree with it field for field — the
    /// dependency is the authority `ADR-0017` chose, standing in until
    /// `M2.25`'s corpus captures real client bytes.
    #[test]
    fn a_batch_the_dependency_encodes_decodes_field_for_field() {
        use kafka_protocol::records::{
            Compression as KpCompression, Record, RecordBatchEncoder, RecordEncodeOptions,
            TimestampType,
        };
        // ⚠️ offset - sequence must match across records or the dependency's
        // encoder splits them into separate batches -- its grouping
        // predicate, found when this test got one batch of one record.
        fn record(offset: i64, key: Option<&'static [u8]>, value: &'static [u8]) -> Record {
            Record {
                transactional: false,
                control: false,
                partition_leader_epoch: 0,
                producer_id: -1,
                producer_epoch: -1,
                timestamp_type: TimestampType::Creation,
                offset,
                sequence: i32::try_from(offset).unwrap_or(0),
                delete_horizon: false,
                timestamp: 1_700_000_000_000 + offset,
                key: key.map(bytes::Bytes::from_static),
                value: Some(bytes::Bytes::from_static(value)),
                headers: kafka_protocol::indexmap::IndexMap::default(),
            }
        }
        let records = vec![record(0, None, b"hello"), record(1, Some(b"k"), b"world")];
        let mut buf = bytes::BytesMut::new();
        RecordBatchEncoder::encode(
            &mut buf,
            &records,
            &RecordEncodeOptions {
                version: 2,
                compression: KpCompression::None,
            },
        )
        .expect("the dependency encodes its own records");

        let header = decode_batch_header(&buf).expect("our decode agrees with its layout");
        assert_eq!(header.base_offset, 0);
        assert_eq!(header.record_count, 2);
        assert_eq!(header.last_offset_delta, 1);
        assert_eq!(header.attributes.compression(), Compression::None);
        assert!(!header.attributes.is_transactional());
        assert_eq!(header.producer_id, -1);
        assert_eq!(
            usize::try_from(header.batch_length).expect("positive") + 12,
            buf.len(),
            "batchLength spans from partitionLeaderEpoch to the end"
        );
    }

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

    #[test]
    fn wrong_magic_is_refused_before_any_field_is_believed() {
        let mut buf = Vec::new();
        encode_batch_header(&mut buf, &sample_header());
        buf[16] = 1; // magic v1
        assert_eq!(
            decode_batch_header(&buf),
            Err(BatchError::WrongMagic { magic: 1 })
        );
    }

    #[test]
    fn a_short_buffer_is_eof_not_a_panic() {
        let mut buf = Vec::new();
        encode_batch_header(&mut buf, &sample_header());
        buf.truncate(20);
        assert!(matches!(
            decode_batch_header(&buf),
            Err(BatchError::Decode(DecodeError::UnexpectedEof { .. }))
        ));
    }
}
