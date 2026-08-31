//! What each S3 method does to the model's state.
//!
//! ⚠️ **Split from `model.rs` at `M10.8`, and the third split in two rows is
//! the signal rather than the limit being tight.** `transport.rs` took the
//! wire, `state.rs` took the data, and what was left was still two things: the
//! request *pipeline* — decode the key, refuse an unmodelled method, apply a
//! fault, dispatch, withhold — and the four handlers the dispatch calls. The
//! pipeline grows with the harness and the handlers grow with S3, which is the
//! seam `code-structure.md` rule 18 says a file over the limit is pointing at.
//!
//! ⚠️ **Fidelity here is the whole product**, which is `model.rs`'s warning and
//! travels with the code it is about: two rounds of review found a branch that
//! could never run with a confident comment beside it — the 416, then the
//! `DELETE` — so a case added here should be reachable through `S3Store` and
//! driven by a test, not merely written.
//!
//! ⚠️ **The surviving-mutant inventory travels with them, and this is the file
//! it is about.** `cargo mutants` generates nothing for `tests/`, verified, so
//! nothing below is protected by a gate and each of these is a branch a green
//! suite would let you break: the GET and HEAD `etag` and `last-modified`
//! headers can be dropped; the 416 branch can be deleted outright — measured,
//! 33 passed — because `truncated_range_error` produces the same error from
//! the success path; HEAD's 404 arm can answer 500; `a >= total` can be
//! weakened to `a >`, since every case uses offset 10 on a 3-byte object and
//! the `start == size` boundary is unexercised; and an `If-Match` against an
//! *absent* key can be made to answer 200, since every case writes the key
//! first.
//!
//! ⚠️ **The list is an inventory of holes, so a *guarded* branch written into
//! it is worse than an omission** — it invites a later editor to write a test
//! that exists and to discount the entries that are real. It also missed two
//! of its own entries on its first draft, which is the argument for not
//! trusting it as a complete inventory either.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]
// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the bare
// `pub` clippy's `redundant_pub_crate` asks for — the trade every file in this
// tree makes, for the same reason.
#![allow(clippy::redundant_pub_crate)]

use std::fmt::Write as _;

use object_store::client::{HttpRequest, HttpResponse};

use super::encoding::xml_unescape;
use super::model::ModelS3;
use super::state::{LAST_MODIFIED, ModelState, Stored};
use super::transport::status;

impl ModelS3 {
    pub(crate) fn put(
        state: &mut ModelState,
        req: &HttpRequest,
        key: String,
        body: Vec<u8>,
    ) -> HttpResponse {
        // ⚠️ **Before `existing` is read, which is what makes it a race.** The
        // competitor lands between the caller's decision and this check, so a
        // conditional write that was correct when it was issued fails here —
        // the window `ADR-0005`'s commit protocol is built to lose in.
        if let Some(bytes) = state.faults.lost_race(&key) {
            state.next_etag += 1;
            let etag = state.next_etag;
            state.objects.insert(key.clone(), Stored { bytes, etag });
        }
        let existing = state.objects.get(&key).cloned();
        // `Precondition::IfAbsent` reaches the wire as `If-None-Match: *`.
        if req.headers().get("if-none-match").is_some() && existing.is_some() {
            return status(412);
        }
        // `Precondition::IfMatches(tok)` reaches it as `If-Match: <etag>`.
        if let Some(want) = req.headers().get("if-match") {
            let want = want
                .to_str()
                .unwrap_or_default()
                .trim_matches('"')
                .to_owned();
            match &existing {
                Some(cur) if cur.etag.to_string() == want => {}
                _ => return status(412),
            }
        }
        state.next_etag += 1;
        let etag = state.next_etag;
        state.objects.insert(key, Stored { bytes: body, etag });
        if state.lose_next_ack {
            state.lose_next_ack = false;
            // ⚠️ **Not a 5xx**: `object_store` retries a server error, and the
            // second attempt would find the flag consumed and succeed — turning
            // "the write happened and the ack was lost" into "the write
            // happened", which is not the case being modelled. A 4xx is
            // answered once.
            //
            // ⚠️ **And not 403, which review measured.** `object_store` maps
            // FORBIDDEN to `PermissionDenied` and `classify` maps that to
            // `Error::Permanent` — `RetryClass::Never` — where
            // `FaultConfig::crash_after_put_before_ack` on the fake gives
            // `Error::Transient`. The one case both backends advertise as
            // running would have produced opposite retry classes, and
            // `a_failed_put_is_not_proof_of_absence` cannot see it because it
            // asserts only `is_err()`. A 400 is answered once *and* classifies
            // as the fake does.
            return status(400);
        }
        let mut resp = status(200);
        resp.headers_mut().insert(
            "etag",
            format!("\"{etag}\"").parse().expect("a valid header"),
        );
        resp
    }

    /// Removes every key the request body names, and answers as S3 does.
    ///
    /// ⚠️ **The response body is not optional**: `object_store` parses a
    /// `DeleteResult` and reports a per-key outcome from it, so a bare 200
    /// leaves the caller with a parse error rather than a delete.
    /// ⚠️ **Absent keys are reported deleted**, which is S3's behaviour and
    /// what makes `conformance.rs`'s `delete_is_idempotent` pass for the right
    /// reason rather than by a retry.
    pub(crate) fn bulk_delete(state: &mut ModelState, body: &[u8]) -> HttpResponse {
        let xml = String::from_utf8_lossy(body);
        let mut out = String::from("<DeleteResult>");
        // The request is `<Delete><Object><Key>k</Key></Object>...</Delete>`.
        for chunk in xml.split("<Key>").skip(1) {
            let Some(raw) = chunk.split("</Key>").next() else {
                continue;
            };
            // ⚠️ Removed by the *decoded* key, echoed with the *raw* one: the
            // map is keyed the way a path decodes, and the response has to be
            // XML `object_store` can parse back.
            let decoded = xml_unescape(raw);
            // ⚠️ **A per-key `<Error>` inside a 200**, which is the shape that
            // makes a bulk delete partially fail: the response is a success as
            // far as HTTP is concerned, and `object_store` reads the outcome
            // per key out of the body. A model that could only fail the whole
            // request could not produce it.
            if let Some(code) = state.faults.refusal(&decoded) {
                write!(
                    out,
                    "<Error><Key>{raw}</Key><Code>{code}</Code>\
                     <Message>injected by the harness</Message></Error>"
                )
                .expect("a String write cannot fail");
                continue;
            }
            state.objects.remove(&decoded);
            write!(out, "<Deleted><Key>{raw}</Key></Deleted>").expect("a String write cannot fail");
        }
        out.push_str("</DeleteResult>");
        let mut resp = HttpResponse::new(out.into_bytes().into());
        *resp.status_mut() = 200u16.try_into().expect("a valid status");
        resp
    }

    /// The object's metadata and no body, which is what a `HEAD` is.
    pub(crate) fn head(state: &ModelState, key: &str) -> HttpResponse {
        let Some(obj) = state.objects.get(key) else {
            return status(404);
        };
        let mut resp = status(200);
        let headers = resp.headers_mut();
        headers.insert(
            "content-length",
            obj.bytes.len().to_string().parse().expect("a valid header"),
        );
        headers.insert(
            "last-modified",
            LAST_MODIFIED.parse().expect("a valid header"),
        );
        headers.insert(
            "etag",
            format!("\"{}\"", obj.etag).parse().expect("a valid header"),
        );
        resp
    }

    pub(crate) fn get(state: &ModelState, req: &HttpRequest, key: &str) -> HttpResponse {
        let Some(obj) = state.objects.get(key) else {
            return status(404);
        };
        let total = obj.bytes.len();
        let requested = req.headers().get("range").map(|raw| {
            let raw = raw.to_str().unwrap_or_default();
            let spec = raw.trim_start_matches("bytes=");
            let (a, b) = spec.split_once('-').unwrap_or(("0", ""));
            let a: usize = a.parse().unwrap_or(0);
            // An HTTP range is inclusive at both ends.
            let b: usize = b.parse().map_or(total, |b: usize| b + 1);
            (a, b)
        });
        // ⚠️ **416, not a clamp**, when the start is past the object: a real S3
        // answers `InvalidRange`, and `oqueue-store` turns that into
        // `ByteRangeOutOfBounds`. ⚠️ **Tested *before* the clamp, which is
        // where the first version put it** — `min(total)` makes `from > total`
        // dead by construction, so the branch was unreachable and a
        // start-past-end read came back `Transient` after eleven retries. The
        // conformance case `M10.3` runs asserts `ByteRangeOutOfBounds`, so the
        // dead branch would have been inherited as a broken case with a
        // comment beside it claiming otherwise.
        // ⚠️ `>=`, not `>`: a first-byte position equal to the length is
        // unsatisfiable under RFC 9110 and S3 answers 416 for it too. At `>`
        // a zero-length slice escaped into the 206 path and the model emitted
        // `content-range: bytes 3-2/3` — an end before its start, which no S3
        // could send. The right error still surfaced, by the HEAD path rather
        // than by this branch.
        if requested.is_some_and(|(a, _)| a >= total) {
            return status(416);
        }
        let (from, to) = requested.map_or((0, total), |(a, b)| (a.min(total), b.min(total)));
        let slice = obj.bytes[from..to].to_vec();
        let len = slice.len();
        let ranged = req.headers().contains_key("range");
        let mut resp = HttpResponse::new(slice.into());
        *resp.status_mut() = if ranged {
            206u16.try_into().expect("a valid status")
        } else {
            200u16.try_into().expect("a valid status")
        };
        // ⚠️ **Not decoration.** `object_store` parses these off the response
        // and fails the read without them — a bare 200 with a body came back
        // as `Transient`, which is the model being wrong rather than the store.
        // `content-range` is what a 206 must carry, and its total is the
        // *object's* size and not the slice's.
        let headers = resp.headers_mut();
        headers.insert(
            "etag",
            format!("\"{}\"", obj.etag).parse().expect("a valid header"),
        );
        headers.insert(
            "content-length",
            len.to_string().parse().expect("a valid header"),
        );
        headers.insert(
            "last-modified",
            LAST_MODIFIED.parse().expect("a valid header"),
        );
        if ranged {
            headers.insert(
                "content-range",
                format!("bytes {from}-{}/{total}", to.saturating_sub(1))
                    .parse()
                    .expect("a valid header"),
            );
        }
        resp
    }
}
