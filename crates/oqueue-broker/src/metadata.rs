//! The `Metadata` answer, v0-v13, against the stub cluster.
//!
//! ⚠️ **`allow_auto_topic_creation` exists from v4** (`M2.md` task 16): at
//! v4+ a requested topic that does not exist is created only when the flag
//! says so, else answered `UNKNOWN_TOPIC_OR_PARTITION` for that topic
//! alone. Below v4 the field does not exist on the wire and the protocol's
//! historical behaviour — creation allowed, the broker's config deciding —
//! is what our decoder's default (`true`) expresses below v4
//! (`oqueue_codec::metadata`).

use crate::stub::StubCluster;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::metadata::{
    MetadataResponse, MetadataResponseTopic, decode_request, encode_response,
};

/// Decodes, answers, encodes. `Close` only when the body cannot be
/// decoded — a malformed request from a client that negotiated fine is a
/// closed connection, same policy as the dispatcher's other unanswerables.
pub(crate) fn handle(
    cluster: &StubCluster,
    prelude: RequestPrelude,
    body: &[u8],
) -> crate::connection::HandlerResponse {
    answer(cluster, prelude, body).map_or(
        crate::connection::HandlerResponse::Close,
        crate::connection::HandlerResponse::Reply,
    )
}

/// One topic resolved to what the response needs, owned so the borrowed
/// [`MetadataResponseTopic`] can point into it.
struct ResolvedTopic {
    name: String,
    topic_id: [u8; 16],
    error_code: i16,
    partition_count: usize,
}

/// The `Option` body `handle` wraps: `None` is the close decision.
fn answer(cluster: &StubCluster, prelude: RequestPrelude, body: &[u8]) -> Option<Vec<u8>> {
    let version = prelude.api_version;
    let request = decode_request(body, version).ok()?;

    // Null asks for every topic this broker has — and so does v0's empty
    // array, which is that version's only all-topics spelling (empty means
    // none only from v1; round 1's review caught the inversion).
    let all_topics = request.topics.is_none()
        || (version == 0 && request.topics.as_ref().is_some_and(Vec::is_empty));
    let names: Option<Vec<String>> = if all_topics {
        Some(cluster.topic_names())
    } else {
        // ⚠️ A null NAME inside an entry (v10+ describe-by-topic-id) is a
        // request this id-less stub cannot serve — refusing beats creating
        // a topic literally named "" and polluting every later response.
        request.topics.as_ref().map_or(Some(Vec::new()), |topics| {
            topics
                .iter()
                .map(|t| t.name.clone())
                .collect::<Option<Vec<String>>>()
        })
    };
    // Same policy as every other unanswerable shape: close.
    let names = names?;

    let resolved: Vec<ResolvedTopic> = names
        .into_iter()
        .map(|name| resolve_topic(cluster, name, version, request.allow_auto_topic_creation))
        .collect();

    let response = MetadataResponse {
        node_id: cluster.node_id,
        host: &cluster.host,
        port: cluster.port,
        cluster_id: Some("oqueue"),
        controller_id: cluster.node_id,
        topics: resolved
            .iter()
            .map(|t| MetadataResponseTopic {
                error_code: t.error_code,
                name: Some(&t.name),
                topic_id: t.topic_id,
                partition_count: t.partition_count,
            })
            .collect(),
    };

    let mut out = Vec::new();
    encode_response_header(&mut out, ApiKey::Metadata, version, prelude.correlation_id).ok()?;
    encode_response(&mut out, version, &response);
    Some(out)
}

/// Resolves one topic: existing topics report their partitions; a missing
/// one is created or refused by the flag. Topic ids ride the wire from v10.
fn resolve_topic(
    cluster: &StubCluster,
    name: String,
    version: i16,
    allow_auto_topic_creation: bool,
) -> ResolvedTopic {
    let exists = cluster.partition_count(&name).is_some();
    if !exists && !allow_auto_topic_creation {
        return ResolvedTopic {
            name,
            topic_id: [0u8; 16],
            error_code: error_codes::UNKNOWN_TOPIC_OR_PARTITION,
            partition_count: 0,
        };
    }
    if !exists {
        cluster.ensure_topic(&name);
    }
    let partition_count = cluster.partition_count(&name).unwrap_or(1);
    let topic_id = if version >= 10 {
        cluster
            .topic_id(&name)
            .map_or([0u8; 16], uuid::Uuid::into_bytes)
    } else {
        [0u8; 16]
    };
    ResolvedTopic {
        name,
        topic_id,
        error_code: error_codes::NONE,
        partition_count,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::handle;
    use crate::stub::StubCluster;
    use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
    use kafka_protocol::messages::{MetadataRequest, MetadataResponse, TopicName};
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
    use oqueue_codec::apikey::ApiKey;
    use oqueue_codec::frame::RequestPrelude;

    fn request_bytes(version: i16, topics: Option<Vec<&str>>, allow_create: bool) -> Vec<u8> {
        let mut request = MetadataRequest::default();
        request.topics = topics.map(|names| {
            names
                .into_iter()
                .map(|n| {
                    let mut t = MetadataRequestTopic::default();
                    t.name = Some(TopicName(StrBytes::from_string(n.to_owned())));
                    t
                })
                .collect()
        });
        request.allow_auto_topic_creation = allow_create;
        let mut out = Vec::new();
        request.encode(&mut out, version).expect("encodes");
        out
    }

    fn prelude(version: i16) -> RequestPrelude {
        RequestPrelude {
            api_key: 3,
            api_version: version,
            correlation_id: 11,
        }
    }

    /// The reply's bytes, or a panic naming the other verdict.
    fn answered(cluster: &StubCluster, prelude: RequestPrelude, body: &[u8]) -> Vec<u8> {
        match handle(cluster, prelude, body) {
            crate::connection::HandlerResponse::Reply(out) => out,
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    fn decode(bytes: &[u8], version: i16) -> MetadataResponse {
        // Metadata responses: header v0 below v9, v1 (tagged) from v9.
        let header_len = if version >= 9 { 5 } else { 4 };
        let mut rest = &bytes[header_len..];
        let r = MetadataResponse::decode(&mut rest, version).expect("decodes");
        assert!(rest.is_empty());
        r
    }

    #[test]
    fn v12_with_the_flag_creates_and_answers() {
        let cluster = StubCluster::new("h.example", 9092);
        let body = request_bytes(12, Some(vec!["orders"]), true);
        let out = answered(&cluster, prelude(12), &body);
        let response = decode(&out, 12);
        assert_eq!(response.brokers.len(), 1);
        assert_eq!(response.brokers[0].port, 9092);
        assert_eq!(response.topics.len(), 1);
        assert_eq!(response.topics[0].error_code, 0);
        assert_eq!(response.topics[0].partitions.len(), 1);
        assert_eq!(cluster.partition_count("orders"), Some(1));
        // From v10 the topic's id rides along -- how id-addressed produce
        // and fetch learn their targets.
        assert_eq!(
            response.topics[0].topic_id,
            cluster.topic_id("orders").expect("created with an id")
        );
        assert_ne!(response.topics[0].topic_id, uuid::Uuid::nil());
    }

    #[test]
    fn v12_without_the_flag_refuses_the_missing_topic() {
        let cluster = StubCluster::new("h", 1);
        let body = request_bytes(12, Some(vec!["ghost"]), false);
        let out = answered(&cluster, prelude(12), &body);
        let response = decode(&out, 12);
        assert_eq!(
            response.topics[0].error_code,
            kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code()
        );
        assert_eq!(
            cluster.partition_count("ghost"),
            None,
            "nothing was created"
        );
    }

    #[test]
    fn a_null_topic_list_answers_everything() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("a");
        cluster.ensure_topic("b");
        let body = request_bytes(12, None, false);
        let out = answered(&cluster, prelude(12), &body);
        let response = decode(&out, 12);
        let mut names: Vec<String> = response
            .topics
            .iter()
            .map(|t| t.name.as_ref().expect("named").to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn below_v4_the_wire_has_no_flag_and_creation_is_the_default() {
        let cluster = StubCluster::new("h", 1);
        // The field is not on the v1 wire at all -- the dependency's encoder
        // refuses a non-default value there, which itself proves the claim --
        // and the decoder defaults it true, the historical behaviour this
        // handler inherits.
        let body = request_bytes(1, Some(vec!["implicit"]), true);
        let out = answered(&cluster, prelude(1), &body);
        let response = decode(&out, 1);
        assert_eq!(response.topics[0].error_code, 0);
        assert_eq!(cluster.partition_count("implicit"), Some(1));
    }

    #[test]
    fn v0_empty_array_means_all_topics() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("v0-visible");
        let body = request_bytes(0, Some(vec![]), true);
        let out = answered(&cluster, prelude(0), &body);
        let response = decode(&out, 0);
        assert_eq!(response.topics.len(), 1, "v0's empty array is all-topics");
    }

    #[test]
    fn from_v1_an_empty_array_means_no_topics() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("hidden");
        let body = request_bytes(12, Some(vec![]), true);
        let out = answered(&cluster, prelude(12), &body);
        let response = decode(&out, 12);
        assert!(response.topics.is_empty());
    }

    /// The whole route through the dispatcher — `supports()` gate, header
    /// decode, body slice — not just the handler (round 1's review noted a
    /// mis-slice would close every connection with nothing failing here).
    #[test]
    fn metadata_routes_through_the_dispatcher() {
        use kafka_protocol::messages::RequestHeader;
        let cluster = std::sync::Arc::new(StubCluster::new("routed.example", 7));
        let dispatcher = crate::Dispatcher::new(std::sync::Arc::clone(&cluster));

        let mut request = Vec::new();
        let mut header = RequestHeader::default();
        header.request_api_key = 3;
        header.request_api_version = 12;
        header.correlation_id = 21;
        header
            .encode(&mut request, ApiKey::Metadata.request_header_version(12))
            .expect("header encodes");
        request.extend_from_slice(&request_bytes(12, Some(vec!["routed"]), true));

        let out = match dispatcher.dispatch(&request) {
            crate::connection::HandlerResponse::Reply(out) => out,
            other => panic!("expected a reply, got {other:?}"),
        };
        let response = decode(&out, 12);
        assert_eq!(response.brokers[0].host.as_str(), "routed.example");
        assert_eq!(response.topics[0].error_code, 0);
        assert_eq!(cluster.partition_count("routed"), Some(1));
    }

    #[test]
    fn the_advertised_identity_is_the_configured_one() {
        let cluster = StubCluster::new("adv.example.test", 31234);
        let body = request_bytes(9, None, false);
        let out = answered(&cluster, prelude(9), &body);
        let response = decode(&out, 9);
        assert_eq!(response.brokers[0].host.as_str(), "adv.example.test");
        assert_eq!(response.brokers[0].port, 31234);
        let _ = ApiKey::Metadata;
    }
}
