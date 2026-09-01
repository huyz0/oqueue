//! `records.rs`'s tests, split out at the 500-line limit (`M10.19`).
//!
//! ⚠️ **Split rather than trimmed**, on `fetch.rs`'s own precedent
//! (`M10.17`, itself following `oqueue-broker/src/produce/`'s `mod.rs` +
//! `tests.rs`): the module boundary the line limit is pointing at is
//! `count_records`/`iter_records` versus the tests that pin them, not any
//! one test being cuttable. `M10.20` is the row that owns splitting this
//! file by *concept* (`RecordIter` from `count_records`); this split moves
//! only the existing test module verbatim, to make room for one more test
//! this task needs without pre-empting that row.

#![allow(clippy::expect_used)]

use super::{RecordsError, iter_records};
use crate::attributes::Compression;
use crate::batch::{BATCH_HEADER_LEN, decode_batch_header};

/// ⚠️ **Counted, against the same authority that wrote them.** The count is
/// what a broker allocates offsets from, so agreeing with the declared
/// `record_count` for a batch the dependency encoded is the whole claim.
#[test]
fn the_records_a_batch_holds_are_counted_by_walking_them() {
    let buf = crate::batch::tests::encoded_two_record_batch();
    assert_eq!(super::count_records(&buf), Ok(2));
    assert_eq!(
        decode_batch_header(&buf).map(|h| h.record_count),
        Ok(2),
        "and the header's claim happens to be honest here"
    );
}

/// ⚠️ **A count is not the header's word for it.** These are batches whose
/// declared count is a lie, with the CRC recomputed exactly as a hostile
/// producer would — the walk must report what is there, not what is said.
#[test]
fn a_declared_count_does_not_change_what_is_counted() {
    for declared in [0_i32, 1, 3, 1000, -1] {
        let mut buf = crate::batch::tests::encoded_two_record_batch();
        buf[57..61].copy_from_slice(&declared.to_be_bytes());
        let crc = crc32c_stand_in(&buf[21..]);
        buf[17..21].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            super::count_records(&buf),
            Ok(2),
            "declared {declared}, holds two either way"
        );
    }
}

/// A CRC this crate can compute without depending on `oqueue-checksum`,
/// which `check-layering.sh` keeps out of it at runtime.
/// ⚠️ Only used to keep a *tampered* fixture self-consistent; nothing here
/// verifies a checksum.
fn crc32c_stand_in(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    !crc
}

/// ⚠️ **A record whose declared length runs past the batch is refused, not
/// counted.** These bytes came off a socket, so the walk must fail rather
/// than read past what was sent (`security.md` rule 3).
#[test]
fn a_record_length_past_the_end_is_refused_rather_than_walked() {
    let mut buf = crate::batch::tests::encoded_two_record_batch();
    // The first record's varint length is the byte right after the header.
    buf[BATCH_HEADER_LEN] = 0xFE;
    assert!(super::count_records(&buf).is_err());
}

/// ⚠️ **A zero-length record is not a record.** The walk skips whatever
/// each length declares, so a `0x00` byte declares nothing, consumes one
/// byte, and counted — N such bytes claimed N records, each of which the
/// coordinator allocates an *offset* for. One byte per offset is not the
/// unbounded lie the count check closed, but it is still a partition whose
/// offsets advance for records carrying no attributes, no timestamp, no key
/// and no value, and a consumer fetching them gets bytes no client decodes.
///
/// ⚠️ **The floor is the format's**: a record body is one byte of
/// attributes and five varints before any key or value at all, which is
/// exactly what `read_one_record` reads.
#[test]
fn a_record_shorter_than_the_format_allows_is_refused_not_counted() {
    for length in 0..6_u8 {
        let mut buf = crate::batch::tests::encoded_two_record_batch();
        buf.truncate(BATCH_HEADER_LEN);
        // One "record" of the declared length, and that many filler bytes.
        buf.push(length << 1); // zigzag: a small non-negative varint
        buf.extend(std::iter::repeat_n(0_u8, usize::from(length)));
        let shorter = i32::try_from(buf.len() - 12).expect("small");
        buf[8..12].copy_from_slice(&shorter.to_be_bytes());
        assert!(
            super::count_records(&buf).is_err(),
            "a {length}-byte record body cannot be a record"
        );
    }
}

/// ⚠️ **And the shortest body that *is* one is counted**, which is the
/// other half: a floor set one too high refuses records a real producer
/// sends, and no client would be able to produce at all.
#[test]
fn the_shortest_record_the_format_can_express_is_counted() {
    let mut buf = crate::batch::tests::encoded_two_record_batch();
    buf.truncate(BATCH_HEADER_LEN);
    buf.push(6 << 1); // a six-byte body: the format's minimum
    // attributes, and five varints of one byte each.
    buf.extend_from_slice(&[0, 0, 0, 1, 1, 0]);
    let shorter = i32::try_from(buf.len() - 12).expect("small");
    buf[8..12].copy_from_slice(&shorter.to_be_bytes());
    assert_eq!(super::count_records(&buf), Ok(1));
}

/// ⚠️ The offset advances per record, not `BATCH_HEADER_LEN` for every
/// one (`M10.19`). Record 1 is fine, record 2 is too short, and the
/// error must name record 2's own length byte.
#[test]
fn a_too_short_record_reports_its_own_offset_not_the_first_records() {
    let mut buf = crate::batch::tests::encoded_two_record_batch();
    buf.truncate(BATCH_HEADER_LEN);
    buf.push(6 << 1); // record 1: zigzag length 6, the format's minimum
    buf.extend_from_slice(&[0, 0, 0, 1, 1, 0]);
    let second_record_offset = 1 + 6; // one varint byte, six body bytes
    buf.push(5 << 1); // record 2: zigzag length 5, one below the floor
    buf.extend_from_slice(&[0, 0, 0, 0, 0]);
    let shorter = i32::try_from(buf.len() - 12).expect("small");
    buf[8..12].copy_from_slice(&shorter.to_be_bytes());
    assert_eq!(
        super::count_records(&buf),
        Err(crate::batch::BatchError::Decode(
            crate::wire::DecodeError::RecordTooShort {
                length: 5,
                min: 6,
                at: BATCH_HEADER_LEN + second_record_offset,
            }
        )),
        "the offset must name record 2's length byte, not record 1's"
    );
}

/// ⚠️ The same advancing offset, for `LengthOutOfBounds` rather than
/// `RecordTooShort` (`M10.19`) — the sibling branch a single mutant
/// (`replace + with -` at this line) survived until this pinned it: with
/// `record_start` always `0` in every other fixture, `+`, `-` and `*` all
/// answered the same `BATCH_HEADER_LEN`. A negative zigzag length (raw
/// varint `0x01` decodes to `-1`) does not fit in `usize`, and record 2's
/// own offset is nonzero, so only `+` gives the right answer.
#[test]
fn a_length_that_does_not_fit_reports_its_own_offset_not_the_first_records() {
    let mut buf = crate::batch::tests::encoded_two_record_batch();
    buf.truncate(BATCH_HEADER_LEN);
    buf.push(6 << 1); // record 1: zigzag length 6, the format's minimum
    buf.extend_from_slice(&[0, 0, 0, 1, 1, 0]);
    let second_record_offset = 1 + 6; // one varint byte, six body bytes
    buf.push(0x01); // record 2: raw varint 1, zigzag-decodes to -1
    let shorter = i32::try_from(buf.len() - 12).expect("small");
    buf[8..12].copy_from_slice(&shorter.to_be_bytes());
    assert_eq!(
        super::count_records(&buf),
        Err(crate::batch::BatchError::Decode(
            crate::wire::DecodeError::LengthOutOfBounds {
                length: 1,
                max: u64::try_from(buf.len() - BATCH_HEADER_LEN).expect("small"),
                at: BATCH_HEADER_LEN + second_record_offset,
            }
        )),
        "the offset must name record 2's length byte, not record 1's"
    );
}

/// ⚠️ **An empty record array counts zero and does not loop.** The `while`
/// is bounded by the array's own end, not by the declared count.
#[test]
fn a_batch_with_no_records_counts_none() {
    let mut buf = crate::batch::tests::encoded_two_record_batch();
    let had = buf.len() - BATCH_HEADER_LEN;
    buf.truncate(BATCH_HEADER_LEN);
    let shorter = i32::try_from(buf.len() - 12).expect("small");
    buf[8..12].copy_from_slice(&shorter.to_be_bytes());
    assert_eq!(super::count_records(&buf), Ok(0));
    assert!(had > 0, "the fixture did have records to remove");
}

/// The records section of the golden dependency-encoded batch —
/// `batch::tests` builds it; this reuses the same authority.
pub(crate) fn golden_records_blob() -> Vec<u8> {
    let batch = crate::batch::tests::encoded_two_record_batch();
    let header = decode_batch_header(&batch).expect("golden decodes");
    assert_eq!(header.attributes.compression(), Compression::None);
    batch[BATCH_HEADER_LEN..].to_vec()
}

#[test]
fn the_golden_batch_iterates_record_for_record() {
    let blob = golden_records_blob();
    let records: Vec<_> = iter_records(&blob)
        .collect::<Result<_, _>>()
        .expect("the dependency's records parse");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].offset_delta, 0);
    assert_eq!(records[0].key, None);
    assert_eq!(records[0].value, Some(&b"hello"[..]));
    assert_eq!(records[1].offset_delta, 1);
    assert_eq!(records[1].key, Some(&b"k"[..]));
    assert_eq!(records[1].value, Some(&b"world"[..]));
    assert_eq!(records[1].timestamp_delta, 1);
}

#[test]
fn a_truncated_record_is_one_error_item_then_silence() {
    let blob = golden_records_blob();
    let truncated = &blob[..blob.len() - 3];
    let items: Vec<_> = iter_records(truncated).collect();
    assert!(items[items.len() - 1].is_err(), "the tail record errors");
    assert_eq!(
        items.iter().filter(|i| i.is_err()).count(),
        1,
        "the iterator poisons after the first error"
    );
}

#[test]
fn a_record_lying_about_its_length_is_a_mismatch_error() {
    let blob = golden_records_blob();
    let mut grown = Vec::new();
    // First record's declared length +1: the inner cursor will have one
    // byte left over.
    let mut cur = crate::wire::Cursor::new(&blob);
    let declared = crate::varint::read_varint(&mut cur).expect("length varint");
    crate::varint::put_varint(&mut grown, declared + 1);
    grown.extend_from_slice(&blob[cur.position()..]);
    let first = iter_records(&grown).next().expect("one item");
    assert!(matches!(
        first,
        Err(RecordsError::RecordLengthMismatch { .. })
    ));
}
