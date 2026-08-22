//! The varint family: every reader, from every cursor position that a
//! record's interior could put one at.
#![no_main]

use libfuzzer_sys::fuzz_target;
use oqueue_codec::wire::Cursor;

fuzz_target!(|data: &[u8]| {
    let _ = oqueue_codec::varint::read_unsigned_varint(&mut Cursor::new(data));
    let _ = oqueue_codec::varint::read_unsigned_varlong(&mut Cursor::new(data));
    let _ = oqueue_codec::varint::read_varint(&mut Cursor::new(data));
    let _ = oqueue_codec::varint::read_varlong(&mut Cursor::new(data));
});
