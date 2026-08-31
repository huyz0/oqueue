//! Undoing what `object_store` does to a key on its way to the wire.
//!
//! ⚠️ **Split from `model.rs` at `M10.6`**, when that file hit the 500-line
//! limit — and on the seam the limit is a signal for: this is about *spelling*
//! a key, where `model.rs` is about what an S3 answers.
//!
//! ⚠️ **The reason both exist is one defect.** A PUT arrives percent-encoded in
//! a URL path and a bulk delete arrives raw in XML, so a key with a space or an
//! `&` was stored under one spelling and deleted under another: the delete
//! answered `<Deleted>` and the object stayed. `ObjectKey::new` accepts any
//! non-empty string, so that is reachable rather than theoretical.

// `pub(crate)` inside a private module — see `model.rs` for the trade.
#![allow(clippy::redundant_pub_crate)]

/// Undoes the percent-encoding `object_store` applies to a key in a URL path.
///
/// ⚠️ **Without it the map is keyed two different ways.** A PUT arrives as a
/// path — `a%20b` — and a bulk delete arrives as XML text — `a b` — so a key
/// with any character outside the unreserved set was stored under one spelling
/// and deleted under another: the delete answered `<Deleted>` and the object
/// stayed. `ObjectKey::new` accepts any non-empty string, so that is reachable
/// rather than theoretical, and every key in `conformance.rs` happens to avoid
/// it, which is why the suite could not see it.
pub(crate) fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &s[i + 1..i + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The five XML entities `object_store` escapes a key with.
pub(crate) fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // ⚠️ Last, or `&amp;lt;` would round-trip to `<` rather than `&lt;`.
        .replace("&amp;", "&")
}
