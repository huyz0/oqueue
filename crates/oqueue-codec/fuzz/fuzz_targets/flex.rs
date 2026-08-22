//! The flexible-versions decoders: no panic, no over-allocation, on any
//! byte string. The compact readers bound every length against the input
//! before allocating (`security.md` rules 1-2) — this drives that promise
//! against arbitrary bytes, the DoS class `M2.26` found in miniature.
#![no_main]

use libfuzzer_sys::fuzz_target;
use oqueue_codec::wire::Cursor;

fuzz_target!(|data: &[u8]| {
    let _ = oqueue_codec::flex::read_compact_nullable_string(&mut Cursor::new(data));
    let _ = oqueue_codec::flex::read_compact_string(&mut Cursor::new(data));
    let _ = oqueue_codec::flex::read_compact_nullable_bytes(&mut Cursor::new(data));
    let _ = oqueue_codec::flex::read_compact_array_len(&mut Cursor::new(data));
    let _ = oqueue_codec::flex::read_tagged_fields(&mut Cursor::new(data));
});
