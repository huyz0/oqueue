//! The RecordBatch v2 header path, exactly as produce composes it: decode,
//! CRC coverage, stored CRC, and the offset rewrite on a mutable copy.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = oqueue_codec::batch::decode_batch_header(data);
    let _ = oqueue_codec::batch::crc_coverage(data);
    let _ = oqueue_codec::batch::stored_crc(data);
    let mut copy = data.to_vec();
    let _ = oqueue_codec::batch::rewrite_base_offset(&mut copy, i64::MAX, i32::MAX);
});
