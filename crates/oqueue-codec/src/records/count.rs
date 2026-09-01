//! Counting a batch's records without decoding one — the produce-path
//! exception `records.rs`'s own module doc used to except itself for
//! (`M10.20`, from `M3.44`).
//!
//! ⚠️ **On the produce path deliberately** (`M3.14`), where the rest of this
//! crate's record-level machinery is opt-in. It decodes no record: it reads
//! each record's varint length and skips that many bytes, so it touches a
//! few bytes per record where [`crate::records::iter_records`] materializes
//! every field. The reason it must run there is that `record_count` is a
//! *claim* inside the CRC-covered region and offsets are allocated from it —
//! see [`count_records`]'s own doc.

use crate::batch::{BATCH_HEADER_LEN, BatchError, CRC_COVERAGE_START, crc_coverage};
use crate::varint::read_varint;
use crate::wire::{Cursor, DecodeError};

/// The shortest record body the format can express.
///
/// ⚠️ **Derived from the fields, not chosen.** A record body is `attributes`
/// (one byte) followed by `timestamp_delta`, `offset_delta`, `key_length`,
/// `value_length` and `header_count` — five varints, each at least one byte —
/// before any key or value bytes at all.
/// [`read_one_record`](super::read_one_record) reads exactly those, so this
/// number moves only if the record format does.
const MIN_RECORD_BODY_LEN: usize = 6;

/// Counts the records a batch actually holds, by walking them.
///
/// ⚠️ **Because `record_count` is a *claim*, and offsets are allocated from
/// it.** The field is inside the CRC-covered region, so a client can declare a
/// thousand records in a batch holding one and have it verify; a broker that
/// believed it would advance the partition's end offset by a thousand and
/// index a range no record occupies. Counting is the only answer that does not
/// rest on the sender.
///
/// The walk is cheap: each v2 record is length-prefixed with a varint, so this
/// reads a length and skips it, never decoding a key, value or header.
///
/// ⚠️ **Uncompressed batches only.** A compressed batch's records are behind a
/// codec this crate does not implement, so counting them is impossible here —
/// the caller decides what to do about that, and refusing is the only answer
/// that does not amount to trusting the field again.
///
/// # Errors
/// [`BatchError::Decode`] if a record's declared length runs past the batch, if
/// the record array is short of `record_count` entries, or if bytes remain
/// after the last one — each of which means the blob and its header disagree.
/// As [`decode_batch_header`](crate::batch::decode_batch_header) for the
/// header itself.
pub fn count_records(batch: &[u8]) -> Result<u32, BatchError> {
    // ⚠️ Through `crc_coverage`, which is what bounds `batchLength` against the
    // buffer — so the slice below cannot run past what the sender actually
    // sent, whatever the header claims.
    let records = crc_coverage(batch)?
        .get(BATCH_HEADER_LEN - CRC_COVERAGE_START..)
        .ok_or(BatchError::Decode(DecodeError::UnexpectedEof {
            needed: BATCH_HEADER_LEN,
            remaining: batch.len(),
            at: BATCH_HEADER_LEN,
        }))?;
    // ⚠️ Not `record_count` as a loop bound — that is the value being checked.
    // The array's own end is what stops this, and a declared count that does
    // not match what was walked is the caller's to refuse.
    let mut cur = Cursor::new(records);
    // ⚠️ `usize`, not `u32`: every iteration consumes at least the varint it
    // read, so this cannot exceed `records.len()` and needs no overflow guard
    // of its own. The narrowing at the end is where a count too large to be a
    // `record_count` becomes an error.
    let mut counted: usize = 0;
    while cur.remaining() > 0 {
        // ⚠️ Before `read_varint`, not after (`M10.19`, from `M3.43`): after
        // would name where the record *body* starts, off by the varint's
        // width -- `M3.37`'s own review refuted that expression. This is
        // the length field's own offset, advancing per record rather than
        // naming `BATCH_HEADER_LEN` for every one.
        let record_start = cur.position();
        let length = read_varint(&mut cur)?;
        let length = usize::try_from(length).map_err(|_| DecodeError::LengthOutOfBounds {
            length: length.unsigned_abs().into(),
            max: u64::try_from(records.len()).unwrap_or(u64::MAX),
            at: BATCH_HEADER_LEN + record_start,
        })?;
        // ⚠️ **A zero-length record is not a record**, and counting one is how
        // a batch of `0x00` bytes became a batch of records. The walk skips
        // what each length declares, so `0x00` declares nothing, consumes one
        // byte, and counts — N such bytes claim N records, each of which the
        // coordinator allocates an *offset* for. A partition's offsets then
        // advance a per-byte rate for records that carry no attributes, no
        // timestamp, no key and no value, and a consumer fetching them is
        // served bytes no client can decode.
        //
        // ⚠️ **The floor is the format's, not a policy**: a record body is one
        // byte of attributes and five varints — timestamp delta, offset delta,
        // key length, value length, header count — before a single byte of key
        // or value, and every varint is at least one byte. `read_one_record`
        // reads exactly those fields, which is where the number comes from.
        if length < MIN_RECORD_BODY_LEN {
            return Err(BatchError::Decode(DecodeError::RecordTooShort {
                length: length as u64,
                min: MIN_RECORD_BODY_LEN as u64,
                at: BATCH_HEADER_LEN + record_start,
            }));
        }
        cur.take(length)?;
        counted += 1;
    }
    u32::try_from(counted).map_err(|_| {
        BatchError::Decode(DecodeError::LengthOutOfBounds {
            length: u64::try_from(counted).unwrap_or(u64::MAX),
            max: u64::from(u32::MAX),
            at: BATCH_HEADER_LEN,
        })
    })
}
