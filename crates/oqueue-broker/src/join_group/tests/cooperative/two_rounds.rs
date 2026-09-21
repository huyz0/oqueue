//! `M4.16`'s first acceptance criterion: no partition is ever delivered to two
//! members in the same generation, across a two-round rebalance.
//!
//! ⚠️ **Its own file because these are the only tests here that sync.** Their
//! siblings ask what the *join* path carries; these drive `JoinGroup` and
//! `SyncGroup` together over two generations, which is the only way to observe
//! a partition moving from one owner to another — and the only way to catch a
//! broker that handed two members the same slice.
//!
//! `two_rounds::liveness` holds the other half of the two-round story: not
//! what a closed round delivers, but which members a round waits for before
//! it closes at all.

#![allow(clippy::expect_used)]

mod liveness;

use super::super::{VERSION, join};
use super::{request_body_offering, round_of};
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::JoinGroupResponse as KpResponse;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;

/// **`M4.16`'s first acceptance criterion**: no partition is ever delivered to
/// two members in the same generation, across a two-round rebalance.
///
/// ⚠️ **What this pins is the broker's half, which is the only half it has.**
/// The assignment is the leader's to compute — this broker never reads the
/// blob — so the property under test is that the broker carries the inputs and
/// outputs faithfully: each member's owned set reaches the leader, and each
/// member receives exactly its own slice and no other. A broker that mixed two
/// members' slices would make the invariant unachievable however correct the
/// assignor was.
///
/// ⚠️ **Two members in one generation, and both rejoining with their real
/// minted ids.** An earlier version of this test put one member in each
/// generation, so its "no partition twice" assertion ran over a single slice
/// every time and could not fail for the reason it claimed; and it built every
/// join with an empty `member_id`, so three *different* members were created
/// and the rejoin path — the one a cooperative client actually uses — was
/// exercised by nothing. Both found by review.
#[tokio::test(start_paused = true)]
async fn a_cooperative_rebalance_never_double_assigns_within_a_generation() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);

    // ── Generation 1: two members, neither owning anything yet. ─────────────
    let first = round_of(&cluster, &[("", b"owns:"), ("", b"owns:")]).await;
    let generation_1 = first[0].generation_id;
    let ids: Vec<String> = first.iter().map(|r| r.member_id.to_string()).collect();
    let leader_1 = first
        .iter()
        .position(|r| !r.members.is_empty())
        .expect("one leader");
    let delivered_1 = deliver(
        &cluster,
        &ids,
        leader_1,
        generation_1,
        &[(ids[0].as_str(), &b"p0"[..]), (ids[1].as_str(), &b"p1"[..])],
    )
    .await;

    let delivered_2 = swap_partitions_in_a_second_generation(&cluster, &ids, generation_1).await;

    assert_no_partition_twice("1", &delivered_1, &["p0", "p1"]);
    assert_no_partition_twice("2", &delivered_2, &["p0", "p1"]);
}

/// Both members rejoin under their own ids carrying what they now own, and the
/// assignor swaps the two partitions over. Answers what the broker delivered.
async fn swap_partitions_in_a_second_generation(
    cluster: &std::sync::Arc<crate::cluster::Cluster>,
    ids: &[String],
    generation_1: i32,
) -> Vec<Vec<u8>> {
    let second = round_of(
        cluster,
        &[(ids[0].as_str(), b"owns:p0"), (ids[1].as_str(), b"owns:p1")],
    )
    .await;
    let generation_2 = second[0].generation_id;
    assert!(
        generation_2 > generation_1,
        "a rebalance advances the generation"
    );
    assert!(
        second.iter().all(|r| r.generation_id == generation_2),
        "both members are answered in the *same* generation, which is what \
         makes 'two members in one round' a thing this test can assert on"
    );
    let leader_2 = second
        .iter()
        .position(|r| !r.members.is_empty())
        .expect("one leader");
    let owned: Vec<&[u8]> = second[leader_2]
        .members
        .iter()
        .map(|m| m.metadata.as_ref())
        .collect();
    assert!(
        owned.contains(&&b"owns:p0"[..]) && owned.contains(&&b"owns:p1"[..]),
        "both owners' sets must reach the leader, or it cannot know what to \
         revoke — got {owned:?}"
    );
    let ids_2: Vec<String> = second.iter().map(|r| r.member_id.to_string()).collect();
    deliver(
        cluster,
        &ids_2,
        leader_2,
        generation_2,
        &[
            (ids_2[0].as_str(), &b"p1"[..]),
            (ids_2[1].as_str(), &b"p0"[..]),
        ],
    )
    .await
}

/// The leader submits `assignment`; every member syncs. Answers each member's
/// own delivered slice, in `ids` order.
async fn deliver(
    cluster: &crate::cluster::Cluster,
    ids: &[String],
    leader: usize,
    generation: i32,
    assignment: &[(&str, &[u8])],
) -> Vec<Vec<u8>> {
    let mut out = vec![Vec::new(); ids.len()];
    let leader_slice = sync(
        cluster,
        sync_body("orders", &ids[leader], generation, assignment),
    )
    .await;
    assert_eq!(leader_slice.error_code, 0, "the leader syncs");
    out[leader] = leader_slice.assignment.to_vec();
    for (i, id) in ids.iter().enumerate() {
        if i == leader {
            continue;
        }
        let slice = sync(cluster, sync_body("orders", id, generation, &[])).await;
        assert_eq!(slice.error_code, 0, "every follower syncs");
        out[i] = slice.assignment.to_vec();
    }
    out
}

/// **A member rejoining an already-open round replaces its own entry, so the
/// leader sees its latest owned set and not a stale one.**
///
/// ⚠️ **This is the one path the other tests here cannot reach.** They rejoin
/// into a *fresh* round, which enrols each member anew; the replace branch
/// fires only when the same member id arrives twice while one round is still
/// collecting — which a cooperative client does whenever it re-sends a join
/// after revoking partitions. Keeping the first entry instead of the second
/// hands the assignor an owned set the member no longer has, so it revokes
/// partitions already given up and leaves the real ones assigned twice. Found
/// by review, as a mutation every other test in this file survived.
///
/// ⚠️ **The ids have to come from a closed round first**, because a minted id
/// is not knowable until the join it was minted for answers, and that answer
/// does not come until the round closes. A first attempt at this test sent
/// three empty `member_id`s and created three *different* members, so the
/// replace branch never ran and the mutation survived it too.
#[tokio::test(start_paused = true)]
async fn a_member_rejoining_an_open_round_updates_its_own_owned_set() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);

    let first = round_of(&cluster, &[("", b"owns:"), ("", b"owns:")]).await;
    let ids: Vec<String> = first.iter().map(|r| r.member_id.to_string()).collect();

    let responses = rejoin_twice_then_close(&cluster, &ids).await;
    let leader = responses
        .iter()
        .find(|r| !r.members.is_empty())
        .expect("one leader");
    let entry = leader
        .members
        .iter()
        .find(|m| m.member_id.as_str() == ids[0])
        .unwrap_or_else(|| panic!("{:?} must be in the roster", ids[0]));
    assert_eq!(
        entry.metadata.as_ref(),
        b"owns:fresh",
        "a member that rejoined an open round must appear once, with its \
         latest owned set — not the one it superseded"
    );
    assert_eq!(
        leader
            .members
            .iter()
            .filter(|m| m.member_id.as_str() == ids[0])
            .count(),
        1,
        "and exactly once, not twice"
    );
}

/// `ids[0]` enrols twice in one round — the second time superseding its own
/// owned set — and `ids[1]` then closes it.
async fn rejoin_twice_then_close(
    cluster: &std::sync::Arc<crate::cluster::Cluster>,
    ids: &[String],
) -> Vec<KpResponse> {
    // `a` reopens a round and enrols with a set it is about to supersede; the
    // round stays open because `b` has not rejoined.
    let stale = {
        let cluster = std::sync::Arc::clone(cluster);
        let body = request_body_offering(
            "orders",
            &ids[0],
            &[("cooperative-sticky", b"owns:stale")],
            VERSION,
        );
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    tokio::task::yield_now().await;
    // The same member again, in the same round: the replace branch.
    let fresh = {
        let cluster = std::sync::Arc::clone(cluster);
        let body = request_body_offering(
            "orders",
            &ids[0],
            &[("cooperative-sticky", b"owns:fresh")],
            VERSION,
        );
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    tokio::task::yield_now().await;
    let closing = {
        let cluster = std::sync::Arc::clone(cluster);
        let body = request_body_offering(
            "orders",
            &ids[1],
            &[("cooperative-sticky", b"owns:other")],
            VERSION,
        );
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let mut responses = Vec::new();
    for joiner in [stale, fresh, closing] {
        responses.push(joiner.await.expect("joins"));
    }
    responses
}

/// One member's own `SyncGroup` body. The leader's carries the whole
/// assignment map; a follower's carries none.
fn sync_body(
    group: &str,
    member_id: &str,
    generation: i32,
    assignments: &[(&str, &[u8])],
) -> Vec<u8> {
    use kafka_protocol::messages::SyncGroupRequest as KpSync;
    use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment as KpAssignment;
    let entries = assignments
        .iter()
        .map(|&(id, bytes)| {
            let mut a = KpAssignment::default();
            a.member_id = StrBytes::from_string(id.to_owned());
            a.assignment = bytes::Bytes::from(bytes.to_vec());
            a
        })
        .collect();
    let request = KpSync::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(generation)
        .with_member_id(StrBytes::from_string(member_id.to_owned()))
        .with_assignments(entries);
    let mut out = Vec::new();
    request.encode(&mut out, 3).expect("encodes");
    out
}

async fn sync(
    cluster: &crate::cluster::Cluster,
    body: Vec<u8>,
) -> kafka_protocol::messages::SyncGroupResponse {
    use kafka_protocol::messages::SyncGroupResponse as KpSync;
    let p = RequestPrelude {
        api_key: 14,
        api_version: 3,
        correlation_id: 7,
    };
    let HandlerResponse::Reply(out) =
        crate::sync_group::handle(cluster, p, &body, &crate::authz::unconfigured_group_authz())
            .await
    else {
        panic!("a SyncGroup replies");
    };
    let mut rest = &out[4..];
    let response = KpSync::decode(&mut rest, 3).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// Every partition across `delivered` appears at most once — the cooperative
/// invariant, checked on the bytes the broker actually handed back.
///
/// ⚠️ **On the delivered bytes, never on the assignment map submitted.** An
/// earlier version checked the literals the test itself wrote, which is the
/// test asserting its own arithmetic: a broker that handed both members the
/// same slice would have passed it.
///
/// ⚠️ **The union is asserted too, not just the absence of duplicates.**
/// Checking only for repeats passes a broker that delivered *nothing*: empty
/// slices have no duplicates. Found by review — mutating the follower path to
/// answer empty bytes left every test here green.
fn assert_no_partition_twice(generation: &str, delivered: &[Vec<u8>], expected: &[&str]) {
    let mut seen: Vec<&str> = delivered
        .iter()
        .flat_map(|s| std::str::from_utf8(s).expect("utf8").split(','))
        .filter(|p| !p.is_empty())
        .collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen.len(),
        before,
        "generation {generation} delivered a partition to two members at once: {delivered:?}"
    );
    let mut wanted: Vec<&str> = expected.to_vec();
    wanted.sort_unstable();
    assert_eq!(
        seen, wanted,
        "generation {generation} must deliver exactly the partitions assigned"
    );
}
