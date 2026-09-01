//! The opt-in record iterator, and the compression seam under it.
//!
//! ⚠️ **Opt-in per code path, never on produce or fetch** (doc 18 §4.4):
//! the broker's hot paths move a batch's records blob as opaque bytes, and
//! [`RecordIter`] exists for the paths that genuinely need record *contents* —
//! `M2.25`'s corpus assertions today, compaction later. Nothing there is
//! reachable from the batch-header layer by accident.
//!
//! ⚠️ **[`count_records`] is the exception, on the produce path
//! deliberately, and lives in its own [`count`] submodule** (`M10.20`, from
//! `M3.44`) rather than excepting itself out of the paragraph above: a
//! module doc that has to except half its file is naming a seam the file
//! itself was not drawn along. See [`count`]'s own doc for why it must run
//! there.
//!
//! ## The compression seam, and where the codecs come from
//!
//! `none`, `lz4` and `zstd` are wired; `gzip` and `snappy` sit behind crate
//! features. Every wired codec is pure Rust — `lz4_flex` both ways,
//! `ruzstd` for decode — because doc 20 §1 budgets no C toolchain, and the
//! decode direction is all M2 needs: a fetch echoes stored batches without
//! recompressing. ⚠️ **zstd *compression* is deliberately absent** — not
//! for lack of a pure-Rust encoder (`ruzstd` 0.9 ships one) but because no
//! path emits zstd before compaction, and unemitted code is unearned; the
//! first emitting path picks between `ruzstd`'s encoder and the C library,
//! with doc 20 §1 weighing on that choice. ⚠️ The lz4 evidence is weaker
//! than the zstd evidence and says so here: the round-trip test is
//! `lz4_flex` against itself, while the zstd fixture crosses
//! implementations — `M2.25`'s corpus, carrying real client bytes, is
//! where lz4 gets its independent check.
//!
//! Every decompression is capped by the caller's byte bound before, during
//! and after — a compressed blob is attacker-supplied, and a zip bomb must
//! die on the bound, not in the allocator.

// Same reasoning `batch.rs` gives for its own shared test helper.
#![allow(clippy::redundant_pub_crate)]

use crate::attributes::Compression;
use crate::varint::{read_varint, read_varlong};
use crate::wire::{Cursor, DecodeError};

mod count;
pub use count::count_records;

/// Why records could not be read.
#[derive(Debug)]
pub enum RecordsError {
    /// The bytes ran out or a bound was violated.
    Decode(DecodeError),
    /// The blob names a codec this build does not carry.
    UnsupportedCodec(Compression),
    /// The codec failed on the bytes themselves.
    Decompress {
        /// The codec that refused.
        codec: Compression,
        /// The codec library's own message.
        detail: String,
    },
    /// Decompression would exceed the caller's bound — the zip-bomb stop.
    DecompressedTooLarge {
        /// The caller's cap in bytes.
        max: u64,
    },
    /// A record's declared length disagrees with its content.
    RecordLengthMismatch {
        /// Bytes the record declared.
        declared: usize,
        /// Bytes its fields actually spanned.
        spanned: usize,
    },
}

impl From<DecodeError> for RecordsError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

impl core::fmt::Display for RecordsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Decode(e) => write!(f, "{e}"),
            Self::UnsupportedCodec(codec) => {
                write!(f, "codec {codec:?} is not wired into this build")
            }
            Self::Decompress { codec, detail } => write!(f, "{codec:?} refused: {detail}"),
            Self::DecompressedTooLarge { max } => {
                write!(f, "decompression exceeds the {max}-byte bound")
            }
            Self::RecordLengthMismatch { declared, spanned } => write!(
                f,
                "record declared {declared} byte(s) but its fields span {spanned}"
            ),
        }
    }
}

impl std::error::Error for RecordsError {}

/// One record, borrowed from the (decompressed) blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordView<'a> {
    /// The record-level attributes byte — unused by the protocol today,
    /// carried verbatim.
    pub attributes: i8,
    /// Milliseconds relative to the batch's `baseTimestamp`.
    pub timestamp_delta: i64,
    /// Offset relative to the batch's `baseOffset`.
    pub offset_delta: i32,
    /// `None` is a null key, distinct from an empty one.
    pub key: Option<&'a [u8]>,
    /// `None` is a null value — a tombstone.
    pub value: Option<&'a [u8]>,
    /// Header key/value pairs, in order.
    pub headers: Vec<(&'a [u8], Option<&'a [u8]>)>,
}

/// Iterates the records of an **uncompressed** blob — run
/// [`crate::compress::decompress_records`] first when the batch's attributes name a codec.
///
/// Yields one `Result` per record and stops after the first error; a
/// trailing partial record is an error item, never a panic and never a
/// silent truncation.
#[must_use]
pub const fn iter_records(blob: &[u8]) -> RecordIter<'_> {
    RecordIter {
        cur: Cursor::new(blob),
        poisoned: false,
    }
}

/// See [`iter_records`].
#[derive(Debug)]
pub struct RecordIter<'a> {
    cur: Cursor<'a>,
    poisoned: bool,
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<RecordView<'a>, RecordsError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.poisoned || self.cur.remaining() == 0 {
            return None;
        }
        let result = read_one_record(&mut self.cur);
        if result.is_err() {
            self.poisoned = true;
        }
        Some(result)
    }
}

/// One record: `length`(varint) then exactly that many bytes of fields.
fn read_one_record<'a>(cur: &mut Cursor<'a>) -> Result<RecordView<'a>, RecordsError> {
    let declared_i32 = read_varint(cur)?;
    let Ok(declared) = usize::try_from(declared_i32) else {
        return Err(RecordsError::Decode(DecodeError::NegativeLength {
            length: declared_i32,
            at: cur.position(),
        }));
    };
    let body = cur.take(declared)?;
    let mut inner = Cursor::new(body);
    let attributes = inner.read_i8()?;
    let timestamp_delta = read_varlong(&mut inner)?;
    let offset_delta = read_varint(&mut inner)?;
    let key = read_sized(&mut inner)?;
    let value = read_sized(&mut inner)?;
    let header_count_i32 = read_varint(&mut inner)?;
    let Ok(header_count) = usize::try_from(header_count_i32) else {
        // Kafka's reference decoder throws on a negative header count;
        // mapping it to zero would accept what every other implementation
        // rejects (`M2.20`'s review, major 1).
        return Err(RecordsError::Decode(DecodeError::NegativeLength {
            length: header_count_i32,
            at: inner.position(),
        }));
    };
    // ⚠️ No preallocation by the declared count: each header costs at least
    // two bytes, so a count the body cannot hold fails on the reads below
    // rather than in the allocator.
    let mut headers = Vec::new();
    for _ in 0..header_count {
        let Some(hkey) = read_sized(&mut inner)? else {
            return Err(RecordsError::Decode(DecodeError::NegativeLength {
                length: -1,
                at: inner.position(),
            }));
        };
        let hval = read_sized(&mut inner)?;
        headers.push((hkey, hval));
    }
    if inner.remaining() != 0 {
        return Err(RecordsError::RecordLengthMismatch {
            declared,
            spanned: declared - inner.remaining(),
        });
    }
    Ok(RecordView {
        attributes,
        timestamp_delta,
        offset_delta,
        key,
        value,
        headers,
    })
}

/// A varint-sized byte field, `-1` null.
fn read_sized<'a>(cur: &mut Cursor<'a>) -> Result<Option<&'a [u8]>, RecordsError> {
    // Field-start offset, `wire.rs`'s convention -- an error points at the
    // length field, not past it.
    let at = cur.position();
    let len_i32 = read_varint(cur)?;
    if len_i32 == -1 {
        return Ok(None);
    }
    let Ok(len) = usize::try_from(len_i32) else {
        return Err(RecordsError::Decode(DecodeError::NegativeLength {
            length: len_i32,
            at,
        }));
    };
    Ok(Some(cur.take(len)?))
}

#[cfg(test)]
pub(crate) mod tests;
