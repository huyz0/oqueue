//! The dispatcher: one decoded frame in, one routed response out — or the
//! decision to close.
//!
//! ⚠️ **`ApiVersions` is answered before anything else exists** (doc 02
//! §7.6): it is the first request every client sends, and its two special
//! behaviours both live here — the response header stays v0 at every body
//! version (the frame layer's golden-pinned case), and a request at a
//! version this broker does not advertise gets the **v0-bodied fallback**:
//! an `ApiVersionsResponse` encoded at v0, `error_code` 35
//! (`UNSUPPORTED_VERSION`), with the `api_keys` table still populated so
//! the client can pick a version and retry. That fallback is `ApiVersions`'
//! alone: any other unanswerable request — an unknown key, an unadvertised
//! version — has no response the client would parse, and the only safe
//! answer is closing the connection: [`HandlerResponse::Close`] from the
//! [`crate::Handler`] seam (ADR-0018's status notes).

use crate::connection::HandlerResponse;
use core::future::Future;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header, read_request_prelude};
use oqueue_codec::versions::supports;

/// Routes decoded frames to API handlers over the cluster.
///
/// `M2.21` built the frame walk and `ApiVersions`; `M2.22` added `Metadata`;
/// `M2.23`/`M2.24` plugged produce and fetch in beside them; `M3.14` pointed
/// those two at the real coordinator, index and object store.
#[derive(Debug)]
pub struct Dispatcher {
    cluster: std::sync::Arc<crate::cluster::Cluster>,
    session: crate::session::Session,
    /// Whether this connection is TLS-terminated — `ADR-0032`'s prerequisite
    /// for `SASL/PLAIN` to ever succeed. `false` by construction
    /// ([`Dispatcher::new`]): the honest default until a composer that
    /// actually knows says otherwise ([`Dispatcher::tls_terminated`]).
    tls: bool,
    /// The `SASL/PLAIN` credentials this connection may authenticate
    /// against (`ADR-0032`, `M9.4`) — empty by construction, matching
    /// `M9.3`'s own shipped "no credential source configured" state.
    credentials: std::sync::Arc<crate::sasl_authenticate::PlainCredentials>,
}

impl Dispatcher {
    /// A dispatcher over `cluster`, and **one session**.
    ///
    /// ⚠️ **Build one per connection.** The session is what carries a
    /// producer's own commit watermark into its next fetch (hazard H2,
    /// `ADR-0023`), and a session is per connection because that is the scope
    /// a client understands: two connections are two clients as far as
    /// read-your-writes is concerned, and sharing one would let an unrelated
    /// client's produce raise the freshness bar for everybody. Sharing the
    /// `Cluster` is the point; sharing the `Dispatcher` is not.
    ///
    /// ⚠️ **Not TLS-terminated, no credentials, by default.** Every one of
    /// this workspace's existing call sites gets exactly `M9.3`'s own
    /// shipped behaviour unchanged: `SASL/PLAIN` refuses every attempt.
    /// [`Dispatcher::tls_terminated`]/[`Dispatcher::with_credentials`] are
    /// additive builder steps a composer that actually knows opts into,
    /// never a silent default this constructor could get wrong.
    #[must_use]
    pub fn new(cluster: std::sync::Arc<crate::cluster::Cluster>) -> Self {
        Self {
            cluster,
            session: crate::session::Session::default(),
            tls: false,
            credentials: std::sync::Arc::default(),
        }
    }

    /// Marks this dispatcher's connection as TLS-terminated.
    ///
    /// ⚠️ **The composer's claim, not this crate's to verify.** Whichever
    /// listener accepted the connection (`crate::tls`'s own capability,
    /// `M9.5`) is the one place that can honestly answer this — `M9.5`'s own
    /// backlog row named wiring a real TLS listener into `bin/oqueue serve`
    /// as separate, not-yet-scoped work; this method is where that answer
    /// will land once it exists.
    #[must_use]
    pub const fn tls_terminated(mut self) -> Self {
        self.tls = true;
        self
    }

    /// The `SASL/PLAIN` credentials this dispatcher's connection may
    /// authenticate against (`ADR-0032`).
    ///
    /// ⚠️ **"From configuration," never invented here.** Reading that
    /// configuration (a file, an environment variable) is `bin/oqueue`'s
    /// job, the composition root — this crate only carries whatever result
    /// it is handed.
    #[must_use]
    pub fn with_credentials(
        mut self,
        credentials: crate::sasl_authenticate::PlainCredentials,
    ) -> Self {
        self.credentials = std::sync::Arc::new(credentials);
        self
    }
}

impl crate::Handler for Dispatcher {
    fn handle(&self, request: Vec<u8>) -> impl Future<Output = HandlerResponse> + Send {
        self.dispatch(request)
    }
}

impl Dispatcher {
    /// The body. ⚠️ **Async because produce and fetch now do I/O**: a produce
    /// PUTs an object and waits for its commit, and a fetch reads objects the
    /// index named. `M2`'s version of this was synchronous with a `ready`
    /// future, and the seam was async for exactly this milestone.
    ///
    /// ⚠️ **It takes the frame by value**, because the future it returns
    /// outlives this call and the connection task owns the buffer.
    pub(crate) async fn dispatch(&self, frame: Vec<u8>) -> HandlerResponse {
        let request: &[u8] = &frame;
        let Ok(prelude) = read_request_prelude(request) else {
            return HandlerResponse::Close;
        };
        let Some(api_key) = ApiKey::from_i16(prelude.api_key) else {
            // An unknown key has no parsable response; close (module doc).
            return HandlerResponse::Close;
        };
        if api_key != ApiKey::ApiVersions && !supports(api_key, prelude.api_version) {
            // Outside ApiVersions' fallback, an unadvertised version has no
            // response the client would parse.
            return HandlerResponse::Close;
        }
        if api_key == ApiKey::ApiVersions {
            return HandlerResponse::Reply(api_versions_response(prelude));
        }
        // The message body starts after the full request header, whose
        // per-API shape `oqueue_codec::frame` now owns (`ADR-0019`).
        let Some(body) = oqueue_codec::frame::decode_request_header(request)
            .ok()
            .map(|(_, consumed)| &request[consumed..])
        else {
            return HandlerResponse::Close;
        };
        match api_key {
            ApiKey::ListOffsets => crate::listoffsets::handle(&self.cluster, prelude, body),
            ApiKey::Metadata => crate::metadata::handle(&self.cluster, prelude, body),
            ApiKey::Produce => {
                crate::produce::handle(&self.cluster, &self.session, prelude, body).await
            }
            ApiKey::Fetch => {
                crate::fetch::handle(&self.cluster, &self.session, prelude, body).await
            }
            ApiKey::InitProducerId => crate::init_producer_id::handle(prelude, body),
            ApiKey::SaslHandshake => crate::sasl_handshake::handle(prelude, body),
            ApiKey::SaslAuthenticate => {
                crate::sasl_authenticate::handle(prelude, body, self.tls, &self.credentials)
            }
            // Answered above by the early return; named rather than a
            // wildcard so an eighth API cannot be silently swallowed here.
            ApiKey::ApiVersions => HandlerResponse::Close,
        }
    }
}

/// The `ApiVersions` answer at any request version, fallback included.
fn api_versions_response(prelude: RequestPrelude) -> Vec<u8> {
    let supported = supports(ApiKey::ApiVersions, prelude.api_version);
    // ⚠️ The fallback's whole point: an unsupported version is answered at
    // v0 — parsable by every client ever shipped — with the error code set
    // and the table still present, so the client retries at a version this
    // broker named (doc 02 §1.4).
    let (body_version, error_code) = if supported {
        (prelude.api_version, error_codes::NONE)
    } else {
        (0, error_codes::UNSUPPORTED_VERSION)
    };

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::ApiVersions,
        body_version,
        prelude.correlation_id,
    )
    .is_err()
    {
        // Unreachable for a correlation-id header; an empty body would at
        // least carry the frame. Kept non-panicking per error-handling.md.
        out.clear();
    }
    oqueue_codec::apiversions::encode_response(&mut out, body_version, error_code);
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::Dispatcher;
    use crate::connection::HandlerResponse;
    use crate::testing::{Fixture, fixture};
    use std::sync::Arc;

    /// ⚠️ The fixture is returned alongside, and holding it is not a
    /// formality: dropping it aborts the coordinator loop, and a dispatcher
    /// over a dead coordinator answers every produce with a refusal.
    async fn dispatcher() -> (Dispatcher, Fixture) {
        let fixture = fixture(&[]).await;
        let dispatcher = Dispatcher::new(Arc::clone(&fixture.cluster));
        (dispatcher, fixture)
    }

    /// The reply's bytes, or a panic naming the other verdict.
    async fn replied(dispatcher: &Dispatcher, request: Vec<u8>) -> Vec<u8> {
        match dispatcher.dispatch(request).await {
            HandlerResponse::Reply(out) => out,
            other => panic!("expected a reply, got {other:?}"),
        }
    }
    use kafka_protocol::messages::ApiVersionsResponse;
    use kafka_protocol::protocol::Decodable;
    use oqueue_codec::wire::{Cursor, put_i16, put_i32};

    fn api_versions_request(version: i16, correlation_id: i32) -> Vec<u8> {
        let mut body = Vec::new();
        put_i16(&mut body, 18);
        put_i16(&mut body, version);
        put_i32(&mut body, correlation_id);
        // client_id: legacy i16-length nullable string, null here.
        put_i16(&mut body, -1);
        if version >= 3 {
            body.push(0x00); // empty tagged fields (v2 request header)
        }
        body
    }

    fn decode_response(bytes: &[u8], body_version: i16) -> (i32, ApiVersionsResponse) {
        // ApiVersions responses carry a v0 header at every version: just
        // the correlation id.
        let mut cur = Cursor::new(bytes);
        let correlation = cur.read_i32().expect("a correlation id");
        let mut rest = &bytes[4..];
        let response =
            ApiVersionsResponse::decode(&mut rest, body_version).expect("a decodable body");
        assert!(rest.is_empty(), "nothing after the body");
        (correlation, response)
    }

    #[tokio::test]
    async fn a_supported_version_is_answered_at_that_version() {
        let (dispatcher, _fixture) = dispatcher().await;
        let out = replied(&dispatcher, api_versions_request(3, 77)).await;
        let (correlation, response) = decode_response(&out, 3);
        assert_eq!(correlation, 77);
        assert_eq!(response.error_code, 0);
        let produce = response
            .api_keys
            .iter()
            .find(|k| k.api_key == 0)
            .expect("produce is advertised");
        assert_eq!((produce.min_version, produce.max_version), (3, 13));
    }

    #[tokio::test]
    async fn an_unsupported_version_gets_the_v0_bodied_fallback() {
        let (dispatcher, _fixture) = dispatcher().await;
        let out = replied(&dispatcher, api_versions_request(99, 5)).await;
        let (correlation, response) = decode_response(&out, 0);
        assert_eq!(correlation, 5);
        assert_eq!(
            response.error_code,
            kafka_protocol::error::ResponseError::UnsupportedVersion.code()
        );
        assert!(
            !response.api_keys.is_empty(),
            "the table rides the fallback so the client can retry"
        );
    }

    #[tokio::test]
    async fn an_unknown_api_key_closes_the_connection() {
        let mut body = Vec::new();
        put_i16(&mut body, 0x7F00);
        put_i16(&mut body, 0);
        put_i32(&mut body, 1);
        let (dispatcher, _fixture) = dispatcher().await;
        assert_eq!(dispatcher.dispatch(body).await, HandlerResponse::Close);
    }

    #[tokio::test]
    async fn an_advertised_api_at_an_unadvertised_version_closes() {
        let mut body = Vec::new();
        put_i16(&mut body, 0); // Produce
        put_i16(&mut body, 2); // removed by KIP-896, below the floor
        put_i32(&mut body, 1);
        let (dispatcher, _fixture) = dispatcher().await;
        assert_eq!(
            dispatcher.dispatch(body).await,
            HandlerResponse::Close,
            "outside ApiVersions' fallback there is nothing parsable to say"
        );
    }

    /// End to end through the connection task: the dispatcher is a Handler,
    /// and the bytes on the wire carry the v0 header both ways.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn api_versions_round_trips_through_a_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (mut client, server) = tokio::io::duplex(4096);
        let limits = crate::ConnectionLimits {
            max_frame: 1024,
            max_in_flight: 4,
            idle_timeout: std::time::Duration::from_hours(1),
        };
        let (dispatcher, _fixture) = dispatcher().await;
        let conn = tokio::spawn(crate::serve_connection(
            server,
            Arc::new(dispatcher),
            limits,
        ));

        let body = api_versions_request(3, 42);
        let mut frame = Vec::new();
        put_i32(&mut frame, i32::try_from(body.len()).expect("small"));
        frame.extend_from_slice(&body);
        client.write_all(&frame).await.expect("request written");

        let mut size = [0u8; 4];
        client.read_exact(&mut size).await.expect("a response size");
        let mut response = vec![0u8; u32::from_be_bytes(size) as usize];
        client.read_exact(&mut response).await.expect("a response");
        let (correlation, decoded) = decode_response(&response, 3);
        assert_eq!(correlation, 42);
        assert_eq!(decoded.error_code, 0);

        drop(client);
        conn.await.expect("joins").expect("clean close");
    }

    /// The close decision travels the whole stack: an unknown api key ends
    /// the connection promptly — the client sees EOF, not a timeout — which
    /// is the seam hole round 1's review found (no test drove a `None`
    /// through `serve_connection`).
    #[tokio::test(start_paused = true)]
    async fn an_unknown_key_closes_the_connection_promptly() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (mut client, server) = tokio::io::duplex(4096);
        let limits = crate::ConnectionLimits {
            max_frame: 1024,
            max_in_flight: 4,
            idle_timeout: std::time::Duration::from_hours(1),
        };
        let (dispatcher, _fixture) = dispatcher().await;
        let conn = tokio::spawn(crate::serve_connection(
            server,
            Arc::new(dispatcher),
            limits,
        ));

        let mut body = Vec::new();
        put_i16(&mut body, 0x7F00);
        put_i16(&mut body, 0);
        put_i32(&mut body, 9);
        let mut frame = Vec::new();
        put_i32(&mut frame, i32::try_from(body.len()).expect("small"));
        frame.extend_from_slice(&body);
        client.write_all(&frame).await.expect("request written");

        // EOF, promptly: under paused time a wrong implementation would sit
        // until the hour-long idle timeout; a correct one closes now.
        let mut buf = [0u8; 1];
        let read = client.read(&mut buf).await.expect("a read completes");
        assert_eq!(read, 0, "the broker closed rather than answering");
        conn.await
            .expect("joins")
            .expect("a clean, deliberate close");
    }
}
