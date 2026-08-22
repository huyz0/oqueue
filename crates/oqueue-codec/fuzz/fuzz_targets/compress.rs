//! Every decompressor behind the seam, bounded: the output cap is the
//! defence (security.md rule 1 -- validate the claim, never allocate it).
#![no_main]

use libfuzzer_sys::fuzz_target;
use oqueue_codec::attributes::Compression;

const MAX_OUT: u64 = 1 << 20;

fuzz_target!(|data: &[u8]| {
    for codec in [
        Compression::None,
        Compression::Gzip,
        Compression::Snappy,
        Compression::Lz4,
        Compression::Zstd,
    ] {
        let _ = oqueue_codec::compress::decompress_records(codec, data, MAX_OUT);
    }
});
