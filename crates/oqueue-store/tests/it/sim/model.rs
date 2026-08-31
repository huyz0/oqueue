//! The deterministic S3 itself: one `HttpService`, answering from a `HashMap`.
//!
//! ⚠️ **Split from `sim.rs` at `M10.2` when the file hit the 500-line limit**,
//! and on the seam the limit is a signal for: this is the *backend* — what an
//! S3 answers to a request — while `sim.rs` is what `S3Store` does when it
//! gets those answers. A change to one is rarely a change to the other.
//!
//! ⚠️ **`cargo mutants` generates nothing for `tests/`**, verified — 145
//! mutants for this crate, every one under `src/` — so no gate constrains this
//! file and the survivors below are recorded here because nothing else will
//! re-establish them. ⚠️ **Each is a branch a green suite would let you
//! break**: the GET and HEAD `etag` and `last-modified` headers can be dropped;
//! the 416 branch can be deleted (`truncated_range_error` produces the same
//! error from the success path); HEAD's 404 arm can answer 500. A change near
//! any of them is unprotected, and `M10.7` — 503 storms, conditional-write
//! races, partial failures — will add more.
//!
//! ⚠️ **Two more the list missed on its first draft**, which is the argument
//! for not trusting it as an inventory: `a >= total` in the 416 branch can be
//! weakened to `a >` (every case uses offset 10 on a 3-byte object, so the
//! `start == size` boundary is unexercised), and an `If-Match` against an
//! *absent* key can be made to answer 200 (every case writes the key first).
//!
//! ⚠️ **Fidelity here is the whole product.** Two rounds of review found a
//! branch that could never run with a confident comment beside it — the 416,
//! then the `DELETE` — so a case added here should be reachable through
//! `S3Store` and driven by a test in `sim.rs`, not merely written.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]
// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the bare
// `pub` clippy's `redundant_pub_crate` asks for. The same trade the broker's
// `region.rs` and `oqueue-core`'s `page.rs` make, for the same reason.
#![allow(clippy::redundant_pub_crate)]

use std::collections::HashMap;
use std::fmt::Write as _;

use super::encoding::{percent_decode, xml_unescape};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use object_store::ClientOptions;
use object_store::client::{HttpConnector, HttpError, HttpRequest, HttpResponse, HttpService};

/// The bucket every request in this file is addressed to.
pub(crate) const BUCKET: &str = "oqueue-sim";

/// ⚠️ **Fixed, because this file is the deterministic one.** A real
/// `Last-Modified` would make two runs of one seed differ in a header
/// `object_store` parses, which is the whole property `M10.4` is built on.
const LAST_MODIFIED: &str = "Wed, 01 Jan 2020 00:00:00 GMT";

/// What the model holds under one key.
#[derive(Clone)]
struct Stored {
    bytes: Vec<u8>,
    /// The entity tag this object would present. ⚠️ A counter rather than a
    /// hash: two writes of identical bytes must produce **different** tags, or
    /// `Precondition::IfMatches` would admit a write it should refuse.
    etag: u64,
}

/// An in-process S3, deterministic and shared by clones.
///
/// ⚠️ **A `std::sync::Mutex` and not an async one, deliberately.** The one
/// `.await` in `respond` collects the request body *before* the lock is taken,
/// so the guard is never held across a suspension point and
/// `async-concurrency.md`'s rule against that is not in play.
#[derive(Debug, Default, Clone)]
pub(crate) struct ModelS3 {
    state: Arc<Mutex<ModelState>>,
}

#[derive(Default)]
struct ModelState {
    objects: HashMap<String, Stored>,
    next_etag: u64,
    /// Per-request delay, when the model was built with one.
    ///
    /// ⚠️ **Off by default**, so every test written before `M10.6` keeps its
    /// meaning: a conformance run that suddenly took simulated minutes would
    /// be a different test, not the same one made realistic.
    latency: Option<super::latency::Latency>,
    /// One-shot: the next PUT stores its bytes and then answers a failure.
    ///
    /// ⚠️ **The thing a real S3 cannot be told to do**, which is why
    /// `s3_minio.rs` declares `injectable_ack_loss: false` and skips
    /// `a_failed_put_is_not_proof_of_absence` — `ADR-0005` guarantee 2, that a
    /// failed write is not proof of absence. A model can be told, so the
    /// simulated backend runs the case S3 must skip.
    lose_next_ack: bool,
}

impl std::fmt::Debug for ModelState {
    /// ⚠️ **Counts, not contents.** A `Debug` that printed every stored object
    /// would put an arbitrary payload in a panic message; the count is what a
    /// reader of a failure actually needs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelState")
            .field("objects", &self.objects.len())
            .field("next_etag", &self.next_etag)
            .field("lose_next_ack", &self.lose_next_ack)
            .field("latency", &self.latency)
            .finish()
    }
}

impl ModelS3 {
    /// The key a request addresses, with the bucket segment removed.
    ///
    /// ⚠️ Path-style, because `AmazonS3Builder::with_virtual_hosted_style_request`
    /// is left off below — so a path is `/<bucket>/<key>` and never a
    /// bucket-qualified host.
    fn key_of(req: &HttpRequest) -> String {
        percent_decode(
            req.uri()
                .path()
                .trim_start_matches('/')
                .trim_start_matches(BUCKET)
                .trim_start_matches('/'),
        )
    }

    async fn respond(&self, req: HttpRequest) -> HttpResponse {
        // ⚠️ **Collected before the lock is taken**, which is what keeps the
        // guard off an `await` — `async-concurrency.md`'s rule. Every method's
        // body is read, not only a PUT's: a GET's is empty and collecting it
        // costs nothing, where a `match` that collected only sometimes would
        // be a second place for the method dispatch to disagree with itself.
        let (req, body) = body_bytes(req).await;
        let key = Self::key_of(&req);
        // ⚠️ **The gap is decided before the lock is taken.** The panic below
        // used to fire while the guard was live, which poisons the mutex — so
        // the next request through any clone died on this `expect`, asserting
        // the opposite of what happened and naming neither method nor URI.
        // That is the same displacement three layers from the gap that the
        // panic replaced a 405 to remove.
        assert!(
            matches!(req.method().as_str(), "PUT" | "GET" | "HEAD" | "DELETE")
                || (req.method() == "POST" && req.uri().query() == Some("delete")),
            "the model has no branch for {} {} — add one rather than teaching a \
             test to expect a backend error",
            req.method(),
            req.uri()
        );
        // ⚠️ **Drawn under the lock, slept outside it**, because a guard held
        // across an `.await` is what `async-concurrency.md` forbids. The lock
        // is not optional in any case: `Latency::draw` takes `&mut self`.
        //
        // ⚠️ **It does not buy determinism, and a first version said it did.**
        // A mutex serializes access without ordering it, so two concurrent
        // requests on a multi-thread runtime take the draws in whichever order
        // the workers arrive — one seed, two runs. What buys determinism is the
        // `current_thread` runtime `ADR-0028` specifies. ⚠️ **So turning
        // latency on for the conformance test, which is `multi_thread`, needs
        // that decided first** — `M10.7` is where it lands.
        let delay = {
            let mut state = self
                .state
                .lock()
                .expect("the model's lock is never poisoned");
            state.latency.as_mut().map(|l| {
                l.draw(if matches!(req.method().as_str(), "PUT" | "POST") {
                    super::latency::Op::Write
                } else {
                    super::latency::Op::Read
                })
            })
        };
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }

        let mut state = self
            .state
            .lock()
            .expect("the model's lock is never poisoned");
        // ⚠️ Matched as a string so this file names no `http` crate — the
        // same discipline `status()` below follows, and the reason neither
        // needs a dependency `object_store` already carries.
        let resp = match req.method().as_str() {
            "PUT" => Self::put(&mut state, &req, key, body),
            "GET" => Self::get(&state, &req, &key),
            // ⚠️ **A HEAD is not optional for this model.** `oqueue-store`
            // answers a failed ranged GET by asking for the object's size and
            // deciding between `ByteRangeOutOfBounds` and a retry from it — so
            // a model that refused HEAD turned a 416 into eleven retries and a
            // `Transient`, which is what the first version did.
            "HEAD" => Self::head(&state, &key),
            // ⚠️ **Reachable only through `object_store`'s single-object
            // path, which `S3Store` does not take** — its `delete` goes to the
            // bulk `POST ?delete` below. Kept because the method is part of
            // what an S3 answers, and marked because review found this arm
            // dead with a confident comment beside it, exactly as it found the
            // 416 branch dead one round earlier.
            "DELETE" => {
                state.objects.remove(&key);
                status(204)
            }
            // ⚠️ **This is how `S3Store` actually deletes.**
            // `ObjectStoreExt::delete` goes through `delete_stream`, which the
            // S3 client turns into one `POST /<bucket>?delete` carrying an XML
            // list — so a model that answered only `DELETE` removed nothing,
            // returned `Transient`, and would have failed three of the
            // conformance cases `M10.3` runs.
            "POST" if req.uri().query() == Some("delete") => Self::bulk_delete(&mut state, &body),
            // ⚠️ **A panic, not a status.** Two earlier versions answered 501
            // and then 405, and both turned a *model gap* into
            // `Error::Transient` — a code the taxonomy defines as a retryable
            // transport blip — three layers from the missing branch, which is
            // how the bulk-delete gap above stayed invisible through a round of
            // review. A panic in a test harness is the test failing, and it
            // names the request that has no branch. It also never reaches
            // `object_store`'s retry layer, which is what the 405 bought.
            method => panic!(
                "the model has no branch for {method} {} — add one rather than \
                 teaching a test to expect a backend error",
                req.uri()
            ),
        };
        // ⚠️ Explicit, because `significant_drop_tightening` is right: the
        // guard has no business outliving the only statement that needs it.
        drop(state);
        resp
    }

    /// Answers every request after a delay drawn from doc 04 §2's curves.
    ///
    /// ⚠️ **Costs no real time only under a paused runtime.** Its one caller
    /// writes `#[tokio::test(start_paused = true)]`; on a plain
    /// `#[tokio::test]` every draw becomes a real `sleep`, which across the
    /// conformance suite's requests would be tens of seconds against NFR-56's
    /// budget. ⚠️ **Not `ADR-0028`'s runtime**, which is
    /// `oqueue-testkit::seeded_runtime` and adds a seeded scheduler this crate
    /// does not depend on — so a run here is paused but not seeded.
    #[must_use]
    pub(crate) fn with_latency(self, seed: u64) -> Self {
        self.state
            .lock()
            .expect("the model's lock is never poisoned")
            .latency = Some(super::latency::Latency::new(seed));
        self
    }

    /// Arms the one-shot above. Cheap enough to be called per case.
    pub(crate) fn arm_ack_loss(&self) {
        self.state
            .lock()
            .expect("the model's lock is never poisoned")
            .lose_next_ack = true;
    }

    fn put(state: &mut ModelState, req: &HttpRequest, key: String, body: Vec<u8>) -> HttpResponse {
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
    fn bulk_delete(state: &mut ModelState, body: &[u8]) -> HttpResponse {
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
            state.objects.remove(&xml_unescape(raw));
            write!(out, "<Deleted><Key>{raw}</Key></Deleted>").expect("a String write cannot fail");
        }
        out.push_str("</DeleteResult>");
        let mut resp = HttpResponse::new(out.into_bytes().into());
        *resp.status_mut() = 200u16.try_into().expect("a valid status");
        resp
    }

    /// The object's metadata and no body, which is what a `HEAD` is.
    fn head(state: &ModelState, key: &str) -> HttpResponse {
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

    fn get(state: &ModelState, req: &HttpRequest, key: &str) -> HttpResponse {
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

/// A response carrying a status and nothing else.
///
/// ⚠️ `HttpResponse::builder()` is unreachable — `builder()` is an inherent
/// method on `Response<()>` and the alias fixes the body type — and
/// `status_mut()`'s signature is what infers `StatusCode` without this file
/// ever naming the `http` crate's type. Both recorded in `testing.md` by
/// `M1.22` so they would not be rediscovered; they were not.
fn status(code: u16) -> HttpResponse {
    let mut resp = HttpResponse::new(Vec::new().into());
    *resp.status_mut() = code.try_into().expect("a valid status");
    resp
}

/// What the store actually sent.
///
/// ⚠️ **`as_bytes()` is not enough, and `testing.md` says otherwise about the
/// *response* body.** `HttpRequestBody` exposes it only for its `Bytes`
/// variant, and a PUT from `object_store` carries a `PutPayload` — so a
/// `body_bytes` built on `as_bytes` stored nothing and the round trip read
/// back empty. `M1.22`'s probe never issued a PUT, which is why the note it
/// left does not cover this.
///
/// The body is fully in memory, so collecting it completes without ever
/// pending; `block_on` here would be a bug and an `await` is what this is.
async fn body_bytes(req: HttpRequest) -> (HttpRequest, Vec<u8>) {
    use http_body_util::BodyExt as _;
    let (parts, body) = req.into_parts();
    let bytes = body
        .collect()
        .await
        .expect("an in-memory body cannot fail to collect")
        .to_bytes()
        .to_vec();
    (HttpRequest::from_parts(parts, Vec::new().into()), bytes)
}

// ⚠️ **The desugared form, so this file adds no `async-trait` dependency** —
// `testing.md` records the probe that established it.
impl HttpService for ModelS3 {
    fn call<'life0, 'async_trait>(
        &'life0 self,
        req: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move { Ok(self.respond(req).await) })
    }
}

impl HttpConnector for ModelS3 {
    fn connect(
        &self,
        _options: &ClientOptions,
    ) -> object_store::Result<object_store::client::HttpClient> {
        Ok(object_store::client::HttpClient::new(self.clone()))
    }
}
