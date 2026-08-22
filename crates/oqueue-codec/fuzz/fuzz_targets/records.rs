//! The record-level iterator over an arbitrary records blob — the opt-in
//! path, and the one a corrupt stored batch would walk.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    for record in oqueue_codec::records::iter_records(data) {
        if record.is_err() {
            break;
        }
    }
});
