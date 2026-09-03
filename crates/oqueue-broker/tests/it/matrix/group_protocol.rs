//! Minimal request bodies and reply decoders for `matrix.rs`'s own FR-2
//! sweep — the six group-protocol APIs (`JoinGroup`, `SyncGroup`,
//! `Heartbeat`, `LeaveGroup`, `OffsetCommit`, `OffsetFetch`) pulled into
//! their own module purely for `code-structure.md`'s five-hundred-line
//! limit, not a different concept from the rest of `matrix.rs`: every
//! function here is `matrix.rs`'s own, called from there.

use kafka_protocol::messages::TopicName;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::apikey::ApiKey;

/// `JoinGroup`'s own minimal body — one member, its own compatible
/// protocol, a rebalance timeout of `1`ms so a fresh group's own round (no
/// early-close signal, `join_group::round`'s own module doc) closes at its
/// own deadline almost immediately rather than holding this test open.
/// Every later version reuses the same group name, so from v1 on the round
/// closes the instant this single member rejoins (`expected` is `Some(1)`
/// from the version before) — its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
pub(super) fn join_group_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::JoinGroupRequest;
    use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol;
    let mut protocol = JoinGroupRequestProtocol::default();
    protocol.name = StrBytes::from_static_str("range");
    protocol.metadata = bytes::Bytes::from_static(b"m");
    let request = JoinGroupRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-group"),
        ))
        .with_session_timeout_ms(1)
        .with_rebalance_timeout_ms(1)
        .with_member_id(StrBytes::from_static_str(""))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    request.encode(out, version).expect("encodes");
}

/// `SyncGroup`'s own minimal body -- a single member submitting its own
/// (non-empty) assignment against a group nothing ever joined, so
/// `M4.11`'s own fencing seam genuinely refuses it `UNKNOWN_MEMBER_ID`
/// (`expected_error_code`'s own row) -- `heartbeat_body`'s own precedent
/// for the same "genuinely served, documented non-zero answer" shape.
pub(super) fn sync_group_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::SyncGroupRequest;
    use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment;
    let mut assignment = SyncGroupRequestAssignment::default();
    assignment.member_id = StrBytes::from_static_str("m1");
    assignment.assignment = bytes::Bytes::from_static(b"a");
    let request = SyncGroupRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-sync-group"),
        ))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_static_str("m1"))
        .with_assignments(vec![assignment]);
    request.encode(out, version).expect("encodes");
}

/// `Heartbeat`'s own minimal body -- against a group nothing ever joined,
/// so `M4.11`'s own fencing seam genuinely answers `UNKNOWN_MEMBER_ID`
/// (`expected_error_code` names this the same way `SaslAuthenticate`'s own
/// row is named: a documented, non-zero, "genuinely served" answer, not
/// `UNSUPPORTED_VERSION`) -- this member was never tracked at all, distinct
/// from the `REBALANCE_IN_PROGRESS` a *tracked* member gets for a stale
/// generation (`heartbeat/tests.rs`'s own coverage of that case).
pub(super) fn heartbeat_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::HeartbeatRequest;
    let request = HeartbeatRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-heartbeat-group"),
        ))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_static_str("m1"));
    request.encode(out, version).expect("encodes");
}

/// `LeaveGroup`'s own minimal body -- one member leaving a group nothing
/// ever joined. The *top-level* `error_code` this test decodes stays `0`
/// unconditionally -- the request itself always succeeds
/// (`leave_group.rs`'s own module doc) -- even though `M4.11`'s own
/// fencing seam now answers this one member `UNKNOWN_MEMBER_ID` in its own
/// per-member entry, which this matrix sweep does not decode
/// (`leave_group/tests.rs`'s own dedicated test covers the per-member
/// code).
pub(super) fn leave_group_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::LeaveGroupRequest;
    let request = if version <= 2 {
        LeaveGroupRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("matrix-leave-group"),
            ))
            .with_member_id(StrBytes::from_static_str("m1"))
    } else {
        use kafka_protocol::messages::leave_group_request::MemberIdentity;
        let mut member = MemberIdentity::default();
        member.member_id = StrBytes::from_static_str("m1");
        LeaveGroupRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("matrix-leave-group"),
            ))
            .with_members(vec![member])
    };
    request.encode(out, version).expect("encodes");
}

/// `OffsetCommit`'s own minimal body -- one partition, against a group
/// nothing ever joined, so `M4.11`'s own fencing seam genuinely answers
/// `UNKNOWN_MEMBER_ID` (`expected_error_code`'s own row) -- `heartbeat_body`'s
/// own precedent for the same "genuinely served, documented non-zero
/// answer" shape.
pub(super) fn offset_commit_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::OffsetCommitRequest;
    use kafka_protocol::messages::offset_commit_request::{
        OffsetCommitRequestPartition, OffsetCommitRequestTopic,
    };
    let mut partition = OffsetCommitRequestPartition::default();
    partition.partition_index = 0;
    partition.committed_offset = 0;
    let mut topic = OffsetCommitRequestTopic::default();
    topic.name = TopicName(StrBytes::from_static_str("t"));
    topic.partitions = vec![partition];
    let request = OffsetCommitRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-offset-commit-group"),
        ))
        .with_generation_id_or_member_epoch(1)
        .with_member_id(StrBytes::from_static_str("m1"))
        .with_topics(vec![topic]);
    request.encode(out, version).expect("encodes");
}

/// `OffsetCommit`'s own decode -- its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
pub(super) fn offset_commit_error_code(rest: &mut &[u8], version: i16) -> i16 {
    kafka_protocol::messages::OffsetCommitResponse::decode(rest, version)
        .expect("OffsetCommit reply decodes")
        .topics[0]
        .partitions[0]
        .error_code
}

/// `JoinGroup`/`SyncGroup`/`Heartbeat`'s own decode -- one function since
/// all three answer a bare `error_code`, its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
pub(super) fn group_protocol_error_code(api_key: ApiKey, rest: &mut &[u8], version: i16) -> i16 {
    match api_key {
        ApiKey::JoinGroup => {
            kafka_protocol::messages::JoinGroupResponse::decode(rest, version)
                .expect("JoinGroup reply decodes")
                .error_code
        }
        ApiKey::SyncGroup => {
            kafka_protocol::messages::SyncGroupResponse::decode(rest, version)
                .expect("SyncGroup reply decodes")
                .error_code
        }
        ApiKey::Heartbeat => {
            kafka_protocol::messages::HeartbeatResponse::decode(rest, version)
                .expect("Heartbeat reply decodes")
                .error_code
        }
        other => unreachable!("group_protocol_error_code called for {other:?}"),
    }
}

/// `OffsetFetch`'s own minimal body -- one partition of `"t"`, explicit
/// (not the null-array all-topics form; `offset_fetch/tests.rs`'s own
/// dedicated tests cover that shape). No fencing (`offset_fetch.rs`'s own
/// module doc: any authenticated client may fetch a group's own committed
/// offsets without joining it), so this genuinely answers `error_code ==
/// 0` -- unlike every other row in this module, which need `M4.11`'s own
/// fencing seam to be told they were never a real member.
pub(super) fn offset_fetch_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::OffsetFetchRequest;
    use kafka_protocol::messages::offset_fetch_request::OffsetFetchRequestTopic;
    let mut topic = OffsetFetchRequestTopic::default();
    topic.name = TopicName(StrBytes::from_static_str("t"));
    topic.partition_indexes = vec![0];
    let request = OffsetFetchRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-offset-fetch-group"),
        ))
        .with_topics(Some(vec![topic]));
    request.encode(out, version).expect("encodes");
}

/// `OffsetFetch`'s own decode -- its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
pub(super) fn offset_fetch_error_code(rest: &mut &[u8], version: i16) -> i16 {
    kafka_protocol::messages::OffsetFetchResponse::decode(rest, version)
        .expect("OffsetFetch reply decodes")
        .topics[0]
        .partitions[0]
        .error_code
}
