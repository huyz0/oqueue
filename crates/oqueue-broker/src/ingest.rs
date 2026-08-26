//! The ingest rule: what makes a produced blob storable.
//!
//! ⚠️ **Its own module because it is a verdict about bytes, not about the
//! `Produce` API**, and because it is the one place untrusted record bytes are
//! judged before anything durable happens. Doc 18 §4.4 designed the batch
//! layout for exactly this: a partition's records must be **one** batch,
//! whole, whose CRC covers every byte after the twelve the broker may rewrite.
//!
//! ⚠️ **v0/v1 batches are refused, never converted** — KIP-110's own
//! precedent, and the error names the format problem rather than steering the
//! client at a compression setting (`M2.md`'s risks).

// ⚠️ Clippy calls the inner `pub(crate)` redundant while `unreachable_pub`
// refuses the alternative — the standoff `produce.rs` and codec's `batch.rs`
// already resolved this way.
#![allow(clippy::redundant_pub_crate)]

use oqueue_codec::attributes::Compression;
use oqueue_codec::batch::{crc_coverage, decode_batch_header, stored_crc};
use oqueue_codec::error_codes;
use oqueue_codec::records::count_records;

/// What one verified batch is worth: how many records it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Verified {
    /// The record count, floored at one — the offsets this batch will occupy.
    pub(crate) records: u32,
}

/// Judges `batch`, returning the Kafka error code to refuse it with.
///
/// ⚠️ **Nothing is stored before this returns `Ok`.** A blob that fails here
/// never reaches a bundle, so bytes the CRC did not cover never reach object
/// storage — which is the whole reason the check is before the write rather
/// than beside it.
pub(crate) fn verify(batch: &[u8]) -> Result<Verified, i16> {
    let header = match decode_batch_header(batch) {
        Ok(header) => header,
        Err(oqueue_codec::batch::BatchError::WrongMagic { .. }) => {
            return Err(error_codes::UNSUPPORTED_FOR_MESSAGE_FORMAT);
        }
        Err(_) => return Err(error_codes::CORRUPT_MESSAGE),
    };
    // Exactly one batch, whole: the header's declared span must be the blob.
    // Longer is a second batch or trailing garbage — bytes the CRC below would
    // never cover, refused rather than stored unverified (real brokers answer
    // multi-batch v3+ produce the same way). Shorter never reaches here:
    // `decode_batch_header` needs 61 bytes and `crc_coverage` refuses a
    // declared end past the blob.
    let declared = usize::try_from(header.batch_length)
        .ok()
        .and_then(|length| length.checked_add(12));
    if declared != Some(batch.len()) {
        return Err(error_codes::INVALID_RECORD);
    }
    // The ingest rule — one line, exactly as `M2.19` shaped it.
    let verified = crc_coverage(batch)
        .ok()
        .zip(stored_crc(batch).ok())
        .is_some_and(|(coverage, stored)| oqueue_checksum::crc32c(coverage) == stored);
    if !verified {
        return Err(error_codes::CORRUPT_MESSAGE);
    }
    // ⚠️ **A compressed batch is refused, not trusted.** Its records are behind
    // a codec nothing here implements, so the count below cannot be checked —
    // and believing the header is the defect the rest of this function exists
    // to close. Decompression is `M8`'s, with the region header's `alg` field.
    // ⚠️ **After the CRC**, deliberately: `attributes` is covered by it, so
    // reading the codec out of an unverified header would be deciding on bytes
    // nothing has vouched for.
    if header.attributes.compression() != Compression::None {
        return Err(error_codes::UNSUPPORTED_COMPRESSION_TYPE);
    }
    // ⚠️ **The declared count is checked against the records, not believed.**
    // `record_count` is inside the CRC-covered region, so a client can declare
    // a thousand records in a batch holding one and have every check above
    // pass — and this count is what offsets are allocated from. A broker that
    // trusted it would advance the partition's end offset by a thousand and
    // index a range no record occupies, so a consumer fetching into that range
    // would get a batch with nothing at its offset, discard it, and re-fetch
    // the same object forever while the watermark advertised records that do
    // not exist. `M2` trusted the field, which was harmless against a stub and
    // is not against a durable log.
    let counted = count_records(batch).map_err(|_| error_codes::CORRUPT_MESSAGE)?;
    if counted == 0 {
        // ⚠️ Refused, not floored to one — which is what `M2` did. A batch with
        // no records occupying an offset is a hole in the log: a consumer that
        // reaches it is served nothing and told the log goes further.
        return Err(error_codes::INVALID_RECORD);
    }
    // ⚠️ Both header fields, because a consumer reads the second: a client
    // advances past a batch by `base_offset + last_offset_delta + 1`, so a
    // batch whose two claims disagree strands whichever consumer believes the
    // smaller one.
    if i64::from(header.record_count) != i64::from(counted)
        || i64::from(header.last_offset_delta) != i64::from(counted) - 1
    {
        return Err(error_codes::INVALID_RECORD);
    }
    Ok(Verified { records: counted })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::verify;
    use crate::testing::{golden_batch, golden_batch_of};
    use oqueue_codec::error_codes;

    #[test]
    fn a_golden_batch_verifies_and_reports_its_record_count() {
        let verified = verify(&golden_batch()).expect("the dependency's own encoder");
        assert_eq!(verified.records, 2);
    }

    #[test]
    fn a_corrupt_crc_is_refused() {
        let mut bad = golden_batch();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert_eq!(verify(&bad), Err(error_codes::CORRUPT_MESSAGE));
    }

    #[test]
    fn an_old_format_batch_names_the_format_problem() {
        let mut old = golden_batch();
        old[16] = 1; // magic v1
        assert_eq!(
            verify(&old),
            Err(error_codes::UNSUPPORTED_FOR_MESSAGE_FORMAT)
        );
    }

    #[test]
    fn a_trailing_byte_or_a_second_batch_is_refused_whole() {
        let mut trailing = golden_batch();
        trailing.push(0xAB);
        let mut doubled = golden_batch();
        doubled.extend_from_slice(&golden_batch());
        for blob in [trailing, doubled] {
            assert_eq!(
                verify(&blob),
                Err(error_codes::INVALID_RECORD),
                "bytes the CRC never covered must not be stored"
            );
        }
    }

    /// ⚠️ **The count is CRC-covered, so a hostile producer can make any value
    /// verify.** Each of these is rewritten in place with the CRC recomputed,
    /// exactly as one would — and each must be refused, because the count is
    /// what offsets are allocated from.
    #[test]
    fn a_record_count_the_batch_does_not_hold_is_refused() {
        // `record_count` is the last four bytes of the 61-byte header;
        // `last_offset_delta` the four at offset 23.
        for (count, delta) in [(-1_i32, 1_i32), (0, -1), (1000, 999), (1, 0), (2, 5)] {
            let mut batch = golden_batch(); // two records
            batch[57..61].copy_from_slice(&count.to_be_bytes());
            batch[23..27].copy_from_slice(&delta.to_be_bytes());
            let crc = oqueue_checksum::crc32c(&batch[21..]);
            batch[17..21].copy_from_slice(&crc.to_be_bytes());
            assert_eq!(
                verify(&batch),
                Err(error_codes::INVALID_RECORD),
                "declared {count} records with last delta {delta}, holds 2"
            );
        }
    }

    /// ⚠️ **And the honest pair is accepted**, so the check above is a check
    /// rather than a refusal of everything.
    #[test]
    fn a_batch_whose_two_claims_match_its_records_is_accepted() {
        let verified = verify(&golden_batch()).expect("two records, declared as two");
        assert_eq!(verified.records, 2);
        let one = verify(&golden_batch_of(&[b"solo"])).expect("one record");
        assert_eq!(one.records, 1);
    }

    /// ⚠️ **What cannot be counted is refused.** A compressed batch's records
    /// are behind a codec nothing here implements, so its declared count could
    /// only be believed — which is the whole defect. `M8` decompresses.
    #[test]
    fn a_compressed_batch_is_refused_rather_than_trusted() {
        let mut batch = golden_batch();
        // `attributes` is the i16 at 21..23, so the codec bits are in byte 22.
        batch[22] |= 0b010; // snappy
        let crc = oqueue_checksum::crc32c(&batch[21..]);
        batch[17..21].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            verify(&batch),
            Err(error_codes::UNSUPPORTED_COMPRESSION_TYPE)
        );
    }

    /// ⚠️ Nothing here may panic: these are bytes a client sent.
    #[test]
    fn a_blob_too_short_to_hold_a_header_is_refused_not_a_panic() {
        assert_eq!(verify(&[]), Err(error_codes::CORRUPT_MESSAGE));
        assert_eq!(verify(&[0; 15]), Err(error_codes::CORRUPT_MESSAGE));
        // ⚠️ Long enough to hold the magic byte, so the format problem is
        // what is named — magic 0 is a v0 batch, refused rather than
        // converted (KIP-110) even though this blob is also too short.
        assert_eq!(
            verify(&[0; 60]),
            Err(error_codes::UNSUPPORTED_FOR_MESSAGE_FORMAT)
        );
    }
}
