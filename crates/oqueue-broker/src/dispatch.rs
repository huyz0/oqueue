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
//! answer is closing the connection, which is what a `None` from the
//! [`crate::Handler`] seam means (ADR-0018's status note).

use core::future::Future;
use kafka_protocol::error::ResponseError;
use kafka_protocol::messages::api_versions_response::ApiVersion;
use kafka_protocol::messages::{ApiKey, ApiVersionsResponse};
use kafka_protocol::protocol::Encodable;
use oqueue_codec::frame::{RequestPrelude, encode_response_header, read_request_prelude};
use oqueue_codec::versions::{ADVERTISED, supports};

/// Routes decoded frames to API handlers over the stub cluster. `M2.21`
/// built the frame walk and `ApiVersions`; `M2.22` added `Metadata`;
/// `M2.23`/`M2.24` plug produce and fetch in beside them.
#[derive(Debug)]
pub struct Dispatcher {
    cluster: std::sync::Arc<crate::stub::StubCluster>,
}

impl Dispatcher {
    /// A dispatcher over `cluster`.
    #[must_use]
    pub const fn new(cluster: std::sync::Arc<crate::stub::StubCluster>) -> Self {
        Self { cluster }
    }
}

impl crate::Handler for Dispatcher {
    fn handle(&self, request: Vec<u8>) -> impl Future<Output = Option<Vec<u8>>> + Send {
        // Nothing here awaits yet (`M2.23`'s produce path will); a ready
        // future keeps the seam's Send bound without an async block clippy
        // would rather see as `async fn`.
        core::future::ready(self.dispatch(&request))
    }
}

impl Dispatcher {
    /// The synchronous body — the seam is async for the APIs that will
    /// await (`M2.23`'s produce path).
    pub(crate) fn dispatch(&self, request: &[u8]) -> Option<Vec<u8>> {
        let prelude = read_request_prelude(request).ok()?;
        let Ok(api_key) = ApiKey::try_from(prelude.api_key) else {
            // An unknown key has no parsable response; close (module doc).
            return None;
        };
        if api_key != ApiKey::ApiVersions && !supports(api_key, prelude.api_version) {
            // Outside ApiVersions' fallback, an unadvertised version has no
            // response the client would parse.
            return None;
        }
        // The message body starts after the full request header, whose
        // shape is per-API and generated -- never computed here.
        let body = || {
            oqueue_codec::frame::decode_request_header(request)
                .ok()
                .map(|(_, consumed)| &request[consumed..])
        };
        match api_key {
            ApiKey::ApiVersions => Some(api_versions_response(prelude)),
            ApiKey::Metadata => crate::metadata::handle(&self.cluster, prelude, body()?),
            // `M2.23`/`M2.24` land here. An advertised API without its
            // handler wired yet is exactly as unanswerable as an
            // unadvertised one.
            _ => None,
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
        (prelude.api_version, 0)
    } else {
        (0, ResponseError::UnsupportedVersion.code())
    };

    let mut response = ApiVersionsResponse::default();
    response.error_code = error_code;
    response.throttle_time_ms = 0;
    response.api_keys = ADVERTISED
        .iter()
        .map(|a| {
            let mut v = ApiVersion::default();
            v.api_key = a.api_key as i16;
            v.min_version = a.min;
            v.max_version = a.max;
            v
        })
        .collect();

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
    if response.encode(&mut out, body_version).is_err() {
        out.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::Dispatcher;
    use crate::stub::StubCluster;
    use std::sync::Arc;

    fn dispatcher() -> Dispatcher {
        Dispatcher::new(Arc::new(StubCluster::new("localhost", 9092)))
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

    #[test]
    fn a_supported_version_is_answered_at_that_version() {
        let out = dispatcher()
            .dispatch(&api_versions_request(3, 77))
            .expect("answered");
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

    #[test]
    fn an_unsupported_version_gets_the_v0_bodied_fallback() {
        let out = dispatcher()
            .dispatch(&api_versions_request(99, 5))
            .expect("the fallback answers");
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

    #[test]
    fn an_unknown_api_key_closes_the_connection() {
        let mut body = Vec::new();
        put_i16(&mut body, 0x7F00);
        put_i16(&mut body, 0);
        put_i32(&mut body, 1);
        assert_eq!(dispatcher().dispatch(&body), None);
    }

    #[test]
    fn an_advertised_api_without_a_handler_yet_closes_too() {
        let mut body = Vec::new();
        put_i16(&mut body, 0); // Produce
        put_i16(&mut body, 9);
        put_i32(&mut body, 1);
        assert_eq!(
            dispatcher().dispatch(&body),
            None,
            "M2.23 wires produce; until then, close"
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
        let conn = tokio::spawn(crate::serve_connection(
            server,
            Arc::new(dispatcher()),
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
        let conn = tokio::spawn(crate::serve_connection(
            server,
            Arc::new(dispatcher()),
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
