//! `M4.16`: `ownedPartitions` round-tripped, and mixed-protocol groups
//! refused.
//!
//! ⚠️ **The broker's half of cooperative rebalancing is a round trip, not an
//! algorithm.** `ownedPartitions` lives inside the subscription metadata blob,
//! which this broker never decodes (`M4.5`) — the assignor is the leader's,
//! and what has to hold here is that every member's owned set reaches that
//! leader unaltered and every member receives exactly its own slice. A broker
//! that dropped an owned set or mixed two slices would make the two-round
//! invariant unachievable however correct the assignor was, and would do it
//! silently, because nothing below the leader reads those bytes.

#![allow(clippy::expect_used)]

mod two_rounds;

use super::{VERSION, join};
use crate::testing::fixture;
use kafka_protocol::messages::JoinGroupRequest as KpRequest;
use kafka_protocol::messages::JoinGroupResponse as KpResponse;
use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
use kafka_protocol::protocol::{Encodable, StrBytes};

/// One member's own `JoinGroup` body, offering several protocol names and
/// carrying arbitrary metadata bytes. `member_id` is `""` for a first join and
/// the minted id for a rejoin.
///
/// ⚠️ **`ownedPartitions` lives *inside* `metadata`, which this broker never
/// decodes** (`M4.5`: subscription metadata is opaque bytes). So a
/// cooperative-sticky client's owned set is carried by the same round trip
/// every other subscription field is, and what `M4.16` has to prove is that
/// the round trip is faithful — not that the broker understands the field.
pub(super) fn request_body_offering_with_timeout(
    group: &str,
    member_id: &str,
    protocols: &[(&str, &[u8])],
    version: i16,
    rebalance_timeout_ms: i32,
) -> Vec<u8> {
    let offered = protocols
        .iter()
        .map(|&(name, metadata)| {
            let mut p = KpProtocol::default();
            p.name = StrBytes::from_string(name.to_owned());
            p.metadata = bytes::Bytes::from(metadata.to_vec());
            p
        })
        .collect();
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_session_timeout_ms(30_000)
        .with_rebalance_timeout_ms(rebalance_timeout_ms)
        .with_member_id(StrBytes::from_string(member_id.to_owned()))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(offered)
        .with_group_instance_id(None);
    let mut out = Vec::new();
    request.encode(&mut out, version).expect("encodes");
    out
}

/// `request_body_offering_with_timeout` at this harness's own default barrier
/// of 10 s — what every case that is not *about* the timeout wants.
pub(super) fn request_body_offering(
    group: &str,
    member_id: &str,
    protocols: &[(&str, &[u8])],
    version: i16,
) -> Vec<u8> {
    request_body_offering_with_timeout(group, member_id, protocols, version, 10_000)
}

/// Joins `members` into one round together and lets that round close on its
/// deadline, answering each one. Every entry is `(member_id, owned metadata)`.
pub(super) async fn round_of(
    cluster: &std::sync::Arc<crate::cluster::Cluster>,
    members: &[(&str, &'static [u8])],
) -> Vec<KpResponse> {
    let joiners: Vec<_> = members
        .iter()
        .map(|&(id, metadata)| {
            let cluster = std::sync::Arc::clone(cluster);
            let body =
                request_body_offering("orders", id, &[("cooperative-sticky", metadata)], VERSION);
            tokio::spawn(async move { join(&cluster, body, VERSION).await })
        })
        .collect();
    for _ in 0..members.len() {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    let mut out = Vec::new();
    for joiner in joiners {
        let response = joiner.await.expect("the joiner joins");
        assert_eq!(response.error_code, 0, "the join succeeds");
        out.push(response);
    }
    out
}

/// **A group whose members share no protocol name is refused, not silently
/// coordinated on one of them.** `M4.16`'s second acceptance criterion.
///
/// ⚠️ **This is the mixed eager/cooperative case, and refusing it is the whole
/// safety argument.** `cooperative-sticky` revokes a partition in one round
/// and hands it over in the next; `range` revokes everything at once. A group
/// running both at the same time has no agreed answer to "who owns this
/// partition right now", which is how the same partition ends up consumed
/// twice. The broker cannot make that safe, so it refuses to form the group.
#[tokio::test(start_paused = true)]
async fn a_group_mixing_range_and_cooperative_sticky_is_refused() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);
    let eager = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body = request_body_offering("orders", "", &[("range", b"eager")], VERSION);
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    tokio::task::yield_now().await;
    let cooperative = join(
        &cluster,
        request_body_offering("orders", "", &[("cooperative-sticky", b"coop")], VERSION),
        VERSION,
    )
    .await;

    assert_eq!(
        cooperative.error_code,
        oqueue_codec::error_codes::INCONSISTENT_GROUP_PROTOCOL,
        "a member sharing no protocol with the round is refused"
    );
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    let eager = eager.await.expect("the eager joiner joins");
    assert_eq!(
        eager.error_code, 0,
        "and the member that was already there is unaffected"
    );
    assert_eq!(
        eager.protocol_name.as_deref(),
        Some("range"),
        "the member that opened the round keeps the protocol it asked for"
    );
}

// ⚠️ **Only one of the two orderings is pinned above, and the other is not
// benign.** `elect` runs over `round.members` — whoever has rejoined *this*
// round — not over the group's known membership, so the protocol is decided
// by whoever opens the round. Reverse the arrival order (the misconfigured
// `cooperative-sticky` consumer opens round two of a stable `range` group)
// and it is both healthy incumbents that are answered
// `INCONSISTENT_GROUP_PROTOCOL`, which the Java consumer raises out of
// `poll()` and never retries. One misconfigured consumer can therefore evict
// a working group.
//
// That is declined scope rather than an oversight: this task's row says
// mixed-protocol groups are unsafe and that it "does not attempt to make them
// safe", and the acceptance criterion asks only that such a group be refused
// rather than silently miscoordinated — which both orderings do. Recorded
// here so the next reader does not mistake the assertion above for a general
// property. Found by review.

/// **Each member's `ownedPartitions` reach the leader under that member's own
/// id.** A cooperative assignor decides what to revoke by reading every
/// member's owned set out of the roster it is handed, so a broker that
/// dropped, truncated, or *transposed* those blobs would make two-round
/// rebalancing impossible — silently, since nothing below the leader reads
/// them.
///
/// ⚠️ **Paired with the id, never a bare multiset.** Checking only the set of
/// blobs passes a rotation — each roster entry carrying the next member's
/// metadata — under which the assignor reads one member's owned set under
/// another's id, revokes partitions that member does not hold, and reassigns
/// partitions the real owner still has. Found by review.
#[tokio::test(start_paused = true)]
async fn owned_partitions_reach_the_leader_under_their_own_member_id() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let owned: [&[u8]; 2] = [b"owns:p0,p1", b"owns:p2"];
    let responses = round_of(&fixture.cluster, &[("", owned[0]), ("", owned[1])]).await;

    let leader: Vec<_> = responses.iter().filter(|r| !r.members.is_empty()).collect();
    assert_eq!(leader.len(), 1, "exactly one response carries the roster");

    // Each response is the answer to the join that sent `owned[i]`, so the
    // response's own minted id is the id that blob must appear under.
    for (response, sent) in responses.iter().zip(owned) {
        let entry = leader[0]
            .members
            .iter()
            .find(|m| m.member_id == response.member_id)
            .unwrap_or_else(|| panic!("{:?} must be in the roster", response.member_id));
        assert_eq!(
            entry.metadata.as_ref(),
            sent,
            "member {:?}'s own owned set must reach the leader under its own id",
            response.member_id
        );
    }
}
