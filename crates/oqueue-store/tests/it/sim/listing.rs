//! `ListObjectsV2`, as the model answers it (`M7.3a`).
//!
//! ⚠️ **Its own file because it is a fifth handler with its own wire format** —
//! a query string in and an XML document out — where `handlers.rs` holds the
//! four that address one key. `S3Store::list` reaches it through
//! `object_store`'s `PaginatedListStore`, and the conformance suite's listing
//! cases drive it, so it is reachable and exercised rather than merely written.
//!
//! ⚠️ **Byte order, string prefix, strict start-after, `max-keys` pages** —
//! what S3 documents, and what the seam promises. A continuation token is the
//! last key of the previous page, which S3 does not promise (its tokens are
//! opaque) but which answers the same next page.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]
// `pub(crate)` inside a private module — see `handlers.rs` for the trade.
#![allow(clippy::redundant_pub_crate)]

use std::fmt::Write as _;

use object_store::client::{HttpRequest, HttpResponse};

use super::encoding::percent_decode;
use super::model::ModelS3;
use super::state::ModelState;

/// Whether `req` is a `ListObjectsV2` rather than an object `GET`.
pub(crate) fn is_list(req: &HttpRequest) -> bool {
    query_param(req, "list-type").as_deref() == Some("2")
}

/// One decoded query parameter, if present.
///
/// ⚠️ `+` is decoded to a space: the query is form-encoded, unlike a path.
fn query_param(req: &HttpRequest, name: &str) -> Option<String> {
    req.uri().query()?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == name).then(|| percent_decode(&v.replace('+', " ")))
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

impl ModelS3 {
    pub(crate) fn list(state: &ModelState, req: &HttpRequest) -> HttpResponse {
        let prefix = query_param(req, "prefix").unwrap_or_default();
        let after =
            query_param(req, "continuation-token").or_else(|| query_param(req, "start-after"));
        let max_keys = query_param(req, "max-keys").map_or(1000, |raw| {
            raw.parse::<usize>().expect("a numeric max-keys")
        });
        let mut keys: Vec<&String> = state
            .objects
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .filter(|key| {
                after
                    .as_ref()
                    .is_none_or(|after| key.as_str() > after.as_str())
            })
            .collect();
        keys.sort_unstable();
        let truncated = keys.len() > max_keys;
        keys.truncate(max_keys);
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult>");
        write!(xml, "<IsTruncated>{truncated}</IsTruncated>").expect("infallible");
        for key in &keys {
            let stored = &state.objects[key.as_str()];
            write!(
                xml,
                "<Contents><Key>{}</Key><LastModified>2020-01-01T00:00:00.000Z</LastModified>\
                 <ETag>\"{}\"</ETag><Size>{}</Size></Contents>",
                xml_escape(key),
                stored.etag,
                stored.bytes.len()
            )
            .expect("infallible");
        }
        if truncated && let Some(last) = keys.last() {
            write!(
                xml,
                "<NextContinuationToken>{}</NextContinuationToken>",
                xml_escape(last)
            )
            .expect("infallible");
        }
        xml.push_str("</ListBucketResult>");
        HttpResponse::new(xml.into_bytes().into())
    }
}
