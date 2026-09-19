//! NFR-12 at scale (`M7.6`): M9's `Metadata` size-and-cost comparison
//! (`tests/it/metadata_cost.rs`, 50 against 500 topics) re-run over a
//! synthetic catalog of 1,000,000 and of 10,000,000 topics.
//!
//! ⚠️ **"Cost" is three counts, never a stopwatch** (`testing.md` rule 11):
//! `Cluster::topic_lookups` (M9's proxy), every call the node makes on its
//! catalog ([`Counted`], which sees a node that pages the whole catalog and
//! truncates where `topic_lookups` cannot), and the allocations one request
//! makes. Each must come out identical at both sizes, as must the response.
//!
//! ⚠️ **Allocations are compared with no tolerance.** Each shape runs on a
//! fresh node after a warm-up, the synthetic names are fixed-width, and the
//! count was identical at both sizes in every run; a difference is therefore
//! work that depends on the catalog, which is what this test exists to find.

use crate::catalog::{Calls, Counted, SyntheticCatalog};
use crate::memory::node;
use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
use kafka_protocol::messages::{
    MetadataRequest, MetadataResponse, RequestHeader, SaslAuthenticateRequest,
};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_broker::{
    Cluster, Dispatcher, Handler, HandlerResponse, PlainCredential, PlainCredentials,
};
use oqueue_codec::apikey::ApiKey;
use oqueue_core::{Principal, Redacted, TopicGrants, TopicId};
use std::sync::Arc;

/// Topics named in the scoped requests, and granted to the principal.
const NAMED: u64 = 50;

const METADATA_VERSION: i16 = 12;

/// What one request cost and answered.
#[derive(Debug, PartialEq, Eq)]
struct Cost {
    reply_bytes: usize,
    topic_lookups: u64,
    catalog: Calls,
    allocations: usize,
}

fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 9;
    header
        .encode(&mut frame, api_key.request_header_version(version))
        .expect("header encodes");
    frame.extend_from_slice(body);
    frame
}

/// A `Metadata` request for `topics`, or for every topic when `None`.
fn metadata_frame(topics: Option<&[String]>) -> Vec<u8> {
    let mut request = MetadataRequest::default();
    request.topics = topics.map(|names| {
        names
            .iter()
            .map(|n| {
                let mut t = MetadataRequestTopic::default();
                t.name = Some(StrBytes::from_string(n.clone()).into());
                t
            })
            .collect()
    });
    let mut body = Vec::new();
    request
        .encode(&mut body, METADATA_VERSION)
        .expect("encodes");
    framed(ApiKey::Metadata, METADATA_VERSION, &body)
}

fn sasl_frame() -> Vec<u8> {
    let mut body = Vec::new();
    SaslAuthenticateRequest::default()
        .with_auth_bytes(bytes::Bytes::from_static(b"\0alice\0alice-secret"))
        .encode(&mut body, 1)
        .expect("encodes");
    framed(ApiKey::SaslAuthenticate, 1, &body)
}

async fn reply(dispatcher: &Dispatcher, frame: Vec<u8>) -> Vec<u8> {
    match dispatcher.handle(frame).await {
        HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    }
}

/// One request's reply and its cost, measured around nothing but the request.
async fn measure(
    dispatcher: &Dispatcher,
    cluster: &Cluster,
    catalog: &Counted<SyntheticCatalog>,
    frame: Vec<u8>,
) -> (Vec<u8>, Cost) {
    let (lookups, calls) = (cluster.topic_lookups(), catalog.calls());
    let allocations = crate::allocations();
    let out = reply(dispatcher, frame).await;
    let allocations = crate::allocations() - allocations;
    let after = catalog.calls();
    let catalog = Calls {
        lookup: after.lookup - calls.lookup,
        lookup_id: after.lookup_id - calls.lookup_id,
        create: after.create - calls.create,
        list: after.list - calls.list,
        listed: after.listed - calls.listed,
    };
    let cost = Cost {
        reply_bytes: out.len(),
        topic_lookups: cluster.topic_lookups() - lookups,
        catalog,
        allocations,
    };
    (out, cost)
}

fn named() -> Vec<String> {
    (0..NAMED).map(SyntheticCatalog::name).collect()
}

/// Which `Metadata` request, and through which dispatcher.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// The same [`NAMED`] topics, by name.
    Named,
    /// Every topic, no credentials configured: bounded by
    /// `MAX_UNSCOPED_TOPICS` since `M7.4`.
    Unscoped,
    /// Every topic, as a principal granted the same [`NAMED`] topics.
    Granted,
}

/// A dispatcher for `shape`, authenticated when the shape needs it.
async fn dispatcher(shape: Shape, cluster: &Arc<Cluster>) -> Dispatcher {
    let open = Dispatcher::new(Arc::clone(cluster));
    let Shape::Granted = shape else {
        return open;
    };
    let alice = Principal::new("alice").expect("valid");
    let mut grants = TopicGrants::new();
    for name in named() {
        grants.grant(alice.clone(), TopicId::new(name).expect("valid"));
    }
    let scoped = open
        .tls_terminated()
        .with_credentials(PlainCredentials::new(vec![PlainCredential {
            principal: alice,
            password: Redacted::new("alice-secret".to_owned()),
        }]))
        .with_topic_grants(grants);
    let _ = reply(&scoped, sasl_frame()).await;
    scoped
}

/// `shape`'s reply and cost on a fresh node over a `size`-topic catalog — a
/// node of its own, so no shape is answered from a cache another filled.
async fn cost_of(shape: Shape, size: u64) -> (Vec<u8>, Cost) {
    let catalog = Arc::new(Counted::new(SyntheticCatalog::new(size)));
    let (cluster, serving) = node(Arc::clone(&catalog) as _).await;
    let dispatcher = dispatcher(shape, &cluster).await;
    let frame = match shape {
        Shape::Named => metadata_frame(Some(&named())),
        Shape::Unscoped | Shape::Granted => metadata_frame(None),
    };
    let measured = measure(&dispatcher, &cluster, &catalog, frame).await;
    serving.abort();
    measured
}

/// Every topic in a reply, sorted by name: a granted set is iterated from a
/// `HashSet` (`tests/it/metadata_cost.rs` says why), and order is not what
/// NFR-12 claims.
fn topics(out: &[u8]) -> Vec<(String, [u8; 16], i16, usize)> {
    let mut rest = &out[5..];
    let response = MetadataResponse::decode(&mut rest, METADATA_VERSION).expect("decodes");
    let mut topics: Vec<_> = response
        .topics
        .iter()
        .map(|t| {
            (
                t.name.as_ref().expect("named").to_string(),
                *t.topic_id.as_bytes(),
                t.error_code,
                t.partitions.len(),
            )
        })
        .collect();
    topics.sort();
    topics
}

#[tokio::test]
async fn metadata_cost_is_flat_between_one_and_ten_million_topics() {
    let _serial = crate::serial().await;
    for shape in [Shape::Named, Shape::Unscoped, Shape::Granted] {
        // Warm-up: one-time growth (the runtime, lazily built statics) lands
        // here, not in whichever size is measured first.
        let _ = cost_of(shape, 1_000).await;
        let (small_out, small) = cost_of(shape, 1_000_000).await;
        let (large_out, large) = cost_of(shape, 10_000_000).await;
        println!("{shape:?}: 1M {small:?}; 10M {large:?}");
        assert_eq!(
            small, large,
            "{shape:?}: a Metadata request's size and cost must not grow with the catalog"
        );
        assert_eq!(
            topics(&small_out),
            topics(&large_out),
            "{shape:?}: the same topics, with the same ids, at both sizes"
        );
        // Paged once and bounded, whatever the catalog holds: a node that
        // listed every name and truncated would show here, where
        // `topic_lookups` alone could not see it.
        let bound = u64::try_from(oqueue_broker::metadata::MAX_UNSCOPED_TOPICS).expect("fits");
        assert!(large.catalog.listed <= bound, "{shape:?}: {large:?}");
    }
}
