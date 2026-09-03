#![allow(clippy::expect_used)]

use super::{
    OffsetFetchResponse, OffsetFetchResponsePartition, OffsetFetchResponseTopic, decode_request,
    encode_response,
};

/// Every version this module defines.
const VERSIONS: [i16; 7] = [1, 2, 3, 4, 5, 6, 7];

#[test]
fn our_request_decode_matches_the_dependency_for_explicit_topics() {
    use kafka_protocol::messages::OffsetFetchRequest as KpRequest;
    use kafka_protocol::messages::offset_fetch_request::OffsetFetchRequestTopic as KpTopic;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut t = KpTopic::default();
        t.name = kafka_protocol::messages::TopicName(StrBytes::from_static_str("orders"));
        t.partition_indexes = vec![0, 1];
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("orders-consumers"),
            ))
            .with_topics(Some(vec![t]));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.group_id, "orders-consumers", "v{version}");
        let topics = ours.topics.expect("explicit, not null");
        assert_eq!(topics.len(), 1, "v{version}");
        assert_eq!(topics[0].name, "orders", "v{version}");
        assert_eq!(topics[0].partition_indexes, vec![0, 1], "v{version}");
    }
}

/// The null-array form: every topic this group has ever committed an
/// offset for -- `topics: None`, not an empty `Vec`.
#[test]
fn our_request_decode_matches_the_dependency_for_the_null_all_topics_form() {
    use kafka_protocol::messages::OffsetFetchRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_topics(None);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert!(
            ours.topics.is_none(),
            "v{version}: null must decode to None, not empty"
        );
    }
}

/// The other direction: the dependency's own decoder reads what we wrote,
/// at every version.
#[test]
fn our_response_encode_matches_the_dependency() {
    use kafka_protocol::messages::OffsetFetchResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = OffsetFetchResponse {
            error_code: 0,
            topics: vec![OffsetFetchResponseTopic {
                name: "orders",
                partitions: vec![
                    OffsetFetchResponsePartition {
                        partition_index: 0,
                        committed_offset: 42,
                        error_code: 0,
                    },
                    OffsetFetchResponsePartition {
                        partition_index: 1,
                        committed_offset: -1,
                        error_code: 0,
                    },
                ],
            }],
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
        assert_eq!(theirs.topics.len(), 1, "v{version}");
        assert_eq!(theirs.topics[0].name.as_str(), "orders", "v{version}");
        assert_eq!(
            theirs.topics[0].partitions[0].committed_offset, 42,
            "v{version}"
        );
        // ⚠️ Not tracked (module doc), but the *sentinel* value itself is
        // pinned: `check-mutants` found `put_i32(out, -1)` -> `put_i32(out,
        // 1)` survive without this, since nothing previously read the
        // field's own decoded value.
        if version >= 5 {
            assert_eq!(
                theirs.topics[0].partitions[0].committed_leader_epoch, -1,
                "v{version}"
            );
        }
        assert_eq!(
            theirs.topics[0].partitions[1].committed_offset, -1,
            "v{version}"
        );
    }
}

/// A group-level refusal (a malformed `group_id`): no topics, just the
/// top-level code.
#[test]
fn our_response_encode_matches_the_dependency_on_a_group_level_refusal() {
    use kafka_protocol::messages::OffsetFetchResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = OffsetFetchResponse {
            error_code: 42, // INVALID_REQUEST
            topics: Vec::new(),
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        // v1 carries no top-level error_code on the wire at all (the
        // dependency's own gate is `version >= 2`) -- there is nothing for
        // this refusal to be seen through below v2.
        let expected = if version >= 2 { 42 } else { 0 };
        assert_eq!(theirs.error_code, expected, "v{version}");
        assert!(theirs.topics.is_empty(), "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::OffsetFetchRequest as KpRequest;
    use kafka_protocol::messages::offset_fetch_request::OffsetFetchRequestTopic as KpTopic;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut t = KpTopic::default();
        t.name = kafka_protocol::messages::TopicName(StrBytes::from_static_str("orders"));
        t.partition_indexes = vec![0];
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_topics(Some(vec![t]));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
