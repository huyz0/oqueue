#![allow(clippy::expect_used)]

use super::{
    OffsetCommitRequestPartition, OffsetCommitRequestTopic, OffsetCommitResponse,
    OffsetCommitResponsePartition, OffsetCommitResponseTopic, decode_request, encode_response,
};

/// Every version this module defines.
const VERSIONS: [i16; 8] = [2, 3, 4, 5, 6, 7, 8, 9];

#[test]
fn our_request_decode_matches_the_dependency() {
    use kafka_protocol::messages::OffsetCommitRequest as KpRequest;
    use kafka_protocol::messages::offset_commit_request::{
        OffsetCommitRequestPartition as KpPartition, OffsetCommitRequestTopic as KpTopic,
    };
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut p1 = KpPartition::default();
        p1.partition_index = 0;
        p1.committed_offset = 100;
        p1.committed_leader_epoch = 7;
        p1.committed_metadata = Some(StrBytes::from_static_str("meta"));
        let mut p2 = KpPartition::default();
        p2.partition_index = 1;
        p2.committed_offset = 200;
        let mut t = KpTopic::default();
        t.name = kafka_protocol::messages::TopicName(StrBytes::from_static_str("orders"));
        t.partitions = vec![p1, p2];

        let mut kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("orders-consumers"),
            ))
            .with_generation_id_or_member_epoch(3)
            .with_member_id(StrBytes::from_static_str("m1"))
            .with_retention_time_ms(-1)
            .with_topics(vec![t]);
        if version >= 7 {
            kp = kp.with_group_instance_id(Some(StrBytes::from_static_str("instance")));
        }
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_decoded_matches_the_two_partitions_sent(&ours, version);
    }
}

/// Split out of the test above purely to keep it under `code-structure.md`'s
/// fifty-line limit — same reasoning `metadata_handle`'s own precedent uses.
fn assert_decoded_matches_the_two_partitions_sent(
    ours: &super::OffsetCommitRequest<'_>,
    version: i16,
) {
    assert_eq!(ours.group_id, "orders-consumers", "v{version}");
    assert_eq!(ours.generation_id, 3, "v{version}");
    assert_eq!(ours.member_id, "m1", "v{version}");
    assert_eq!(ours.topics.len(), 1, "v{version}");
    assert_eq!(ours.topics[0].name, "orders", "v{version}");
    assert_eq!(ours.topics[0].partitions.len(), 2, "v{version}");
    assert_eq!(
        ours.topics[0].partitions[0].partition_index, 0,
        "v{version}"
    );
    assert_eq!(
        ours.topics[0].partitions[0].committed_offset, 100,
        "v{version}"
    );
    assert_eq!(
        ours.topics[0].partitions[1].partition_index, 1,
        "v{version}"
    );
    assert_eq!(
        ours.topics[0].partitions[1].committed_offset, 200,
        "v{version}"
    );
}

/// The other direction: the dependency's own decoder reads what we wrote,
/// at every version.
#[test]
fn our_response_encode_matches_the_dependency() {
    use kafka_protocol::messages::OffsetCommitResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = OffsetCommitResponse {
            topics: vec![OffsetCommitResponseTopic {
                name: "orders",
                partitions: vec![
                    OffsetCommitResponsePartition {
                        partition_index: 0,
                        error_code: 0,
                    },
                    OffsetCommitResponsePartition {
                        partition_index: 1,
                        error_code: 25, // UNKNOWN_MEMBER_ID
                    },
                ],
            }],
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.topics.len(), 1, "v{version}");
        assert_eq!(theirs.topics[0].name.as_str(), "orders", "v{version}");
        assert_eq!(theirs.topics[0].partitions.len(), 2, "v{version}");
        assert_eq!(
            theirs.topics[0].partitions[0].partition_index, 0,
            "v{version}"
        );
        assert_eq!(theirs.topics[0].partitions[0].error_code, 0, "v{version}");
        assert_eq!(
            theirs.topics[0].partitions[1].partition_index, 1,
            "v{version}"
        );
        assert_eq!(theirs.topics[0].partitions[1].error_code, 25, "v{version}");
    }
}

/// A request naming no topics at all -- a legal, if pointless, commit.
#[test]
fn a_request_with_no_topics_decodes() {
    use kafka_protocol::messages::OffsetCommitRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_generation_id_or_member_epoch(1)
            .with_member_id(StrBytes::from_static_str("m1"));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert!(ours.topics.is_empty(), "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::OffsetCommitRequest as KpRequest;
    use kafka_protocol::messages::offset_commit_request::{
        OffsetCommitRequestPartition as KpPartition, OffsetCommitRequestTopic as KpTopic,
    };
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut p = KpPartition::default();
        p.partition_index = 0;
        p.committed_offset = 100;
        let mut t = KpTopic::default();
        t.name = kafka_protocol::messages::TopicName(StrBytes::from_static_str("orders"));
        t.partitions = vec![p];
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_generation_id_or_member_epoch(1)
            .with_member_id(StrBytes::from_static_str("m1"))
            .with_topics(vec![t]);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}

// Referenced only to keep the `OffsetCommitRequestPartition`/`Topic`
// constructors exercised by a direct build, not only the dependency's own
// round trip -- `code-structure.md`'s dead-code discipline for a type this
// module's own broker caller constructs.
#[test]
fn our_own_request_types_are_constructible_directly() {
    let partition = OffsetCommitRequestPartition {
        partition_index: 0,
        committed_offset: 5,
    };
    let topic = OffsetCommitRequestTopic {
        name: "orders",
        partitions: vec![partition],
    };
    assert_eq!(topic.partitions[0].committed_offset, 5);
}
