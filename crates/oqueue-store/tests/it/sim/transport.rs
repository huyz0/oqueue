//! How an answer reaches `object_store`, as distinct from what the answer is.
//!
//! ⚠️ **Split from `model.rs` at `M10.7`, on the seam the 500-line limit is a
//! signal for** — the same move `M10.2` made when `sim.rs` hit it. `model.rs`
//! is what an S3 *answers*; this is the plumbing that carries it: building a
//! bare response, collecting a request body, and the two `object_store` traits
//! that make the model reachable as an HTTP client at all. A change to one is
//! rarely a change to the other, and the fault framework this row adds is
//! entirely on the answering side.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]
// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the bare
// `pub` clippy's `redundant_pub_crate` asks for — the trade `model.rs` and
// `latency.rs` both make beside it, for the same reason.
#![allow(clippy::redundant_pub_crate)]

use std::future::Future;
use std::pin::Pin;

use object_store::ClientOptions;
use object_store::client::{HttpConnector, HttpError, HttpRequest, HttpResponse, HttpService};

use super::model::ModelS3;

/// A response carrying a status and nothing else.
///
/// ⚠️ `HttpResponse::builder()` is unreachable — `builder()` is an inherent
/// method on `Response<()>` and the alias fixes the body type — and
/// `status_mut()`'s signature is what infers `StatusCode` without this file
/// ever naming the `http` crate's type. Both recorded in `testing.md` by
/// `M1.22` so they would not be rediscovered; they were not.
pub(crate) fn status(code: u16) -> HttpResponse {
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
pub(crate) async fn body_bytes(req: HttpRequest) -> (HttpRequest, Vec<u8>) {
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
