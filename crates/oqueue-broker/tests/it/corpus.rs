//! The golden-byte corpus: real client request frames, decoded and
//! re-encoded byte-exactly (`M2.md` item 18, `m2-complete.sh`'s corpus leg).
//!
//! ⚠️ **Provenance is the point.** Every `.hex` file under `tests/corpus/`
//! was captured from librdkafka 2.15.0 talking to `oqueue serve` through a
//! recording proxy (`scripts/harness/capture-proxy.py`) — the file name
//! records client and version. These are the bytes a real client sends, not
//! bytes our own encoder round-tripped into existence; a fixture the
//! encoder generated would only prove the encoder agrees with itself.
//!
//! Two claims per frame: our frame layer plus the generated message layer
//! **decode** it (a real client's opening bytes are never unanswerable),
//! and re-encoding the decoded form reproduces the input **byte for byte**
//! (nothing was dropped, defaulted, or reordered on the way through).

#![allow(clippy::expect_used)]

use kafka_protocol::messages::{
    ApiVersionsRequest, FetchRequest, MetadataRequest, ProduceRequest, RequestHeader,
};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::frame::decode_request_header;

fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/");
    let hex = std::fs::read_to_string(format!("{path}{name}.hex")).expect("fixture exists");
    let hex = hex.trim();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Three claims per real frame, on the M2.30-owned header and the still-
/// borrowed body:
/// 1. **Our** header decode extracts the right fields and body offset.
/// 2. It agrees with the `kafka-protocol` oracle on those fields (the
///    `ADR-0019` differential, on bytes a real client actually sent).
/// 3. The oracle round-trips the whole frame byte-for-byte — the golden
///    guarantee that the captured bytes are what they claim.
fn assert_reencodes<T: Decodable + Encodable>(name: &str, api_key: ApiKey) {
    let frame = fixture(name);

    // (1) Our header decode.
    let (ours, consumed) = decode_request_header(&frame).expect("our header decodes");
    assert_eq!(ours.api_key, api_key.as_i16());
    let version = ours.api_version;
    let header_version = api_key.request_header_version(version);

    // (2) The oracle decodes the same header; agree on fields and offset.
    let mut cur: &[u8] = &frame;
    let kp_header = RequestHeader::decode(&mut cur, header_version).expect("oracle header decodes");
    assert_eq!(consumed, frame.len() - cur.len(), "{name}: body offset");
    assert_eq!(ours.correlation_id, kp_header.correlation_id);
    assert_eq!(ours.client_id.as_deref(), kp_header.client_id.as_deref());

    // (3) The oracle round-trips header + body byte-for-byte.
    let request = T::decode(&mut cur, version).expect("body decodes");
    assert!(cur.is_empty(), "the whole frame is consumed");
    let mut reencoded = Vec::new();
    kp_header
        .encode(&mut reencoded, header_version)
        .expect("header re-encodes");
    request
        .encode(&mut reencoded, version)
        .expect("body re-encodes");
    assert_eq!(reencoded, frame, "{name}: byte-exact re-encode");
}

#[test]
fn librdkafka_api_versions_reencodes_byte_exactly() {
    assert_reencodes::<ApiVersionsRequest>("apiversions-v3", ApiKey::ApiVersions);
}

#[test]
fn librdkafka_metadata_reencodes_byte_exactly() {
    assert_reencodes::<MetadataRequest>("metadata-v13", ApiKey::Metadata);
}

#[test]
fn librdkafka_produce_reencodes_byte_exactly() {
    assert_reencodes::<ProduceRequest>("produce-v10", ApiKey::Produce);
}

#[test]
fn librdkafka_fetch_reencodes_byte_exactly() {
    assert_reencodes::<FetchRequest>("fetch-v16", ApiKey::Fetch);
}

/// Every corpus frame gets an answer — real client bytes are never met
/// with a closed connection, driven through the public [`Handler`] seam.
/// (The produce frame carries acks=-1 — librdkafka's default — so a reply;
/// the fetch frame names a topic id from the capture session that this
/// fresh stub never issued, and the answer is still a reply — the
/// per-partition refusal `M2.24` pinned.)
#[tokio::test]
async fn every_corpus_frame_is_answered_not_closed() {
    use oqueue_broker::Handler;
    let cluster = std::sync::Arc::new(oqueue_broker::StubCluster::new("h", 1));
    cluster.ensure_topic("harness");
    let dispatcher = oqueue_broker::Dispatcher::new(cluster);
    for name in ["apiversions-v3", "metadata-v13", "produce-v10", "fetch-v16"] {
        let frame = fixture(name);
        assert!(
            matches!(
                dispatcher.handle(frame).await,
                oqueue_broker::HandlerResponse::Reply(_)
            ),
            "{name}: a real client frame must be answered"
        );
    }
}
