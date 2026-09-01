//! `count_records`, driven directly on arbitrary bytes (`M10.20`, from
//! `M3.44`).
//!
//! ⚠️ **The two other targets that touch this walk both miss its interesting
//! states.** `request` reaches it through `ingest::verify`, but that
//! function checks the batch's stored CRC-32C *before* calling
//! `count_records`, so a mutated frame dies there and coverage never gets
//! past unmutated corpus batches; `records` drives `iter_records`, a
//! different decoder entirely, and never calls this one. `count_records`
//! itself never checks the checksum — `crc_coverage` only bounds
//! `batchLength` against the buffer — so calling it directly reaches every
//! branch (`RecordTooShort`, `LengthOutOfBounds`, the trailing `UnexpectedEof`)
//! without needing a valid CRC to get there.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = oqueue_codec::records::count_records(data);
});
