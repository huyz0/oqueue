//! The request frame decoders: no panic, no allocation-before-validation,
//! on any byte string a client could send (security.md rules 1, 3).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = oqueue_codec::frame::read_request_prelude(data);
    let _ = oqueue_codec::frame::decode_request_header(data);
});
