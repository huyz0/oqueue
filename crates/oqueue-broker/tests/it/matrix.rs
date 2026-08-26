//! FR-2: every advertised API version is genuinely served.
//!
//! `ApiVersions` advertises a table; this test holds the broker to it.
//! For every `(api, version)` pair in `ADVERTISED`, a minimal valid
//! request through the public [`Handler`] seam gets a real answer —
//! never a closed connection, never error 35 — which is
//! `m2-complete.sh`'s "the matrix is honest" leg.

#![allow(clippy::expect_used)]

use crate::support::broker;
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{
    ApiVersionsRequest, FetchRequest, MetadataRequest, ProduceRequest, RequestHeader, TopicName,
};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_broker::{Cluster, Dispatcher, Handler, HandlerResponse};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::versions::ADVERTISED;
use std::sync::Arc;

/// A full request frame: header at the API's header version, then `body`.
fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 99;
    header
        .encode(&mut frame, api_key.request_header_version(version))
        .expect("header encodes");
    frame.extend_from_slice(body);
    frame
}

/// The smallest valid body for `api_key` at `version`, against a cluster
/// that has topic `"t"` — enough for a real answer, not an error dance.
fn minimal_body(api_key: ApiKey, version: i16, cluster: &Cluster) -> Vec<u8> {
    let mut body = Vec::new();
    match api_key {
        ApiKey::ApiVersions => {
            ApiVersionsRequest::default()
                .encode(&mut body, version)
                .expect("encodes");
        }
        ApiKey::Metadata => {
            MetadataRequest::default()
                .encode(&mut body, version)
                .expect("encodes");
        }
        ApiKey::Produce => {
            let mut request = ProduceRequest::default();
            request.acks = -1;
            let mut topic = TopicProduceData::default();
            if version >= 13 {
                topic.topic_id = cluster.topic_id("t").expect("t exists");
            } else {
                topic.name = TopicName(StrBytes::from_static_str("t"));
            }
            let mut partition = PartitionProduceData::default();
            partition.index = 0;
            partition.records = Some(bytes::Bytes::from(golden_batch()));
            topic.partition_data.push(partition);
            request.topic_data.push(topic);
            request.encode(&mut body, version).expect("encodes");
        }
        ApiKey::Fetch => {
            let mut request = FetchRequest::default();
            request.max_wait_ms = 0;
            let mut topic = FetchTopic::default();
            if version >= 13 {
                topic.topic_id = cluster.topic_id("t").expect("t exists");
            } else {
                topic.topic = TopicName(StrBytes::from_static_str("t"));
            }
            let mut partition = FetchPartition::default();
            partition.partition = 0;
            partition.partition_max_bytes = 1 << 20;
            topic.partitions.push(partition);
            request.topics.push(topic);
            request.encode(&mut body, version).expect("encodes");
        }
    }
    body
}

/// Decode the reply body with the client half at the request's version —
/// a reply that only *looks* like one (wrong header, mis-versioned body)
/// fails here rather than passing as opaque bytes. Returns `ApiVersions`'
/// top-level error code, 0 for the APIs that have none.
fn decode_reply(api_key: ApiKey, version: i16, reply: &[u8]) -> i16 {
    // Response header: v0 is 4 bytes of correlation id, v1 adds the empty
    // tagged-fields byte. ApiVersions stays v0 at every version — the
    // generated `response_header_version` carries that special case.
    let header_len = if api_key.response_header_version(version) >= 1 {
        5
    } else {
        4
    };
    let mut rest = &reply[header_len..];
    let error_code = match api_key {
        ApiKey::ApiVersions => {
            kafka_protocol::messages::ApiVersionsResponse::decode(&mut rest, version)
                .expect("ApiVersions reply decodes")
                .error_code
        }
        ApiKey::Metadata => {
            kafka_protocol::messages::MetadataResponse::decode(&mut rest, version)
                .expect("Metadata reply decodes");
            0
        }
        ApiKey::Produce => {
            kafka_protocol::messages::ProduceResponse::decode(&mut rest, version)
                .expect("Produce reply decodes");
            0
        }
        ApiKey::Fetch => {
            kafka_protocol::messages::FetchResponse::decode(&mut rest, version)
                .expect("Fetch reply decodes");
            0
        }
    };
    assert!(
        rest.is_empty(),
        "{api_key:?} v{version}: nothing after the body"
    );
    error_code
}

#[tokio::test]
async fn every_advertised_version_is_served() {
    let broker = broker(&["t"]).await;
    let cluster = Arc::clone(&broker.cluster);
    let dispatcher = Dispatcher::new(Arc::clone(&cluster));

    for advertised in ADVERTISED {
        for version in advertised.min..=advertised.max {
            let body = minimal_body(advertised.api_key, version, &cluster);
            let frame = framed(advertised.api_key, version, &body);
            let reply = match dispatcher.handle(frame).await {
                HandlerResponse::Reply(reply) => reply,
                other => panic!(
                    "{:?} v{version}: not answered: {other:?}",
                    advertised.api_key
                ),
            };
            let error_code = decode_reply(advertised.api_key, version, &reply);
            assert_eq!(
                error_code, 0,
                "{:?} v{version}: an advertised version is never error 35",
                advertised.api_key
            );
        }
    }
}

/// The dependency's own encoder building the batch produce needs — the
/// same authority as the corpus and the unit fixtures (`ADR-0017`).
fn golden_batch() -> Vec<u8> {
    use kafka_protocol::records::{
        Compression, Record, RecordBatchEncoder, RecordEncodeOptions, TimestampType,
    };
    let record = Record {
        transactional: false,
        control: false,
        partition_leader_epoch: 0,
        producer_id: -1,
        producer_epoch: -1,
        timestamp_type: TimestampType::Creation,
        offset: 0,
        sequence: 0,
        delete_horizon: false,
        timestamp: 1_700_000_000_000,
        key: None,
        value: Some(bytes::Bytes::from_static(b"matrix")),
        headers: kafka_protocol::indexmap::IndexMap::default(),
    };
    let mut buf = bytes::BytesMut::new();
    RecordBatchEncoder::encode(
        &mut buf,
        &[record],
        &RecordEncodeOptions {
            version: 2,
            compression: Compression::None,
        },
    )
    .expect("the dependency encodes its own records");
    buf.to_vec()
}
