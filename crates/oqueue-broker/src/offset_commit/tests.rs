#![allow(clippy::expect_used)]

use super::handle;
use crate::authz::AuthzContext;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::OffsetCommitRequest as KpRequest;
use kafka_protocol::messages::OffsetCommitResponse as KpResponse;
use kafka_protocol::messages::offset_commit_request::{
    OffsetCommitRequestPartition as KpPartition, OffsetCommitRequestTopic as KpTopic,
};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;
use oqueue_core::{
    CommitVersion, GroupMetadataEntry, GroupMetadataLog, GroupMetadataRecord, Principal,
    TopicGrants, TopicId,
};
use std::sync::Arc;

const VERSION: i16 = 8;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 8,
        api_version: VERSION,
        correlation_id: 1,
    }
}

/// Credentials off: every commit fails open, `testing.rs`'s own default
/// `AuthzContext` shape.
fn open() -> AuthzContext<'static> {
    AuthzContext {
        principal: None,
        credentials_configured: false,
        topic_grants: &EMPTY_GRANTS,
    }
}

static EMPTY_GRANTS: std::sync::LazyLock<TopicGrants> = std::sync::LazyLock::new(TopicGrants::new);

fn body(group: &str, generation: i32, member_id: &str, topics: &[(&str, &[i32])]) -> Vec<u8> {
    let kp_topics = topics
        .iter()
        .map(|&(name, partitions)| {
            let mut t = KpTopic::default();
            t.name = kafka_protocol::messages::TopicName(StrBytes::from_string(name.to_owned()));
            t.partitions = partitions
                .iter()
                .map(|&index| {
                    let mut p = KpPartition::default();
                    p.partition_index = index;
                    p.committed_offset = 42;
                    p
                })
                .collect();
            t
        })
        .collect();
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id_or_member_epoch(generation)
        .with_member_id(StrBytes::from_string(member_id.to_owned()))
        .with_topics(kp_topics);
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

async fn commit(
    cluster: &crate::cluster::Cluster,
    body: &[u8],
    authz: &AuthzContext<'_>,
) -> KpResponse {
    let HandlerResponse::Reply(out) = handle(cluster, prelude(), body, authz).await else {
        panic!("an OffsetCommit replies");
    };
    let mut rest = &out[5..]; // v8 is flexible: a 5-byte response header.
    let response = KpResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// Registers `member_id` with `heartbeat.rs`'s own tracking and drives the
/// coordinator to `Stable` at generation `1` — `sync_group/tests.rs`'s own
/// `seat` helper, the same shape.
fn seat(cluster: &crate::cluster::Cluster, group: &str, member_id: &str) {
    let g = oqueue_core::GroupId::new(group).expect("valid");
    cluster.heartbeats().register(&g, member_id, 30_000);
    cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::Join)
        .expect("Empty -> Join is legal");
    cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::JoinBarrierComplete)
        .expect("PreparingRebalance -> CompletingRebalance is legal");
    cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");
}

/// ⚠️ **`M4.11`'s own fencing seam, reused rather than reinvented** — a
/// member this broker never tracked is refused `UNKNOWN_MEMBER_ID`, echoed
/// on every named topic/partition (the wire has no top-level error field
/// of its own to answer with instead).
#[tokio::test(start_paused = true)]
async fn an_untracked_member_is_refused_unknown_member_id() {
    let fixture = fixture(&["orders"]).await;
    let response = commit(
        &fixture.cluster,
        &body("g", 1, "ghost", &[("orders", &[0])]),
        &open(),
    )
    .await;
    assert_eq!(response.topics.len(), 1);
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
}

/// A tracked member naming a stale generation is refused
/// `ILLEGAL_GENERATION`, distinct from `UNKNOWN_MEMBER_ID` — the same
/// distinction `heartbeat/tests.rs` already proves for `Heartbeat`.
#[tokio::test(start_paused = true)]
async fn a_tracked_member_with_a_stale_generation_is_refused_illegal_generation() {
    let fixture = fixture(&["orders"]).await;
    seat(&fixture.cluster, "g", "m1");
    let response = commit(
        &fixture.cluster,
        &body("g", 999, "m1", &[("orders", &[0])]),
        &open(),
    )
    .await;
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::ILLEGAL_GENERATION
    );
}

/// A commit this broker cannot fence at all (no such group has ever
/// existed) is refused the same way an untracked member is -- there is no
/// coordinator record to distinguish "unknown group" from "unknown
/// member" against.
#[tokio::test(start_paused = true)]
async fn a_commit_against_a_group_nothing_ever_joined_is_refused() {
    let fixture = fixture(&["orders"]).await;
    let response = commit(
        &fixture.cluster,
        &body("never-joined", 1, "m1", &[("orders", &[0])]),
        &open(),
    )
    .await;
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
}

/// A fenced, well-formed commit actually lands -- readable back through
/// `CommittedOffsets`' own test-only accessor.
#[tokio::test(start_paused = true)]
async fn a_fenced_commit_actually_lands() {
    let fixture = fixture(&["orders"]).await;
    seat(&fixture.cluster, "g", "m1");
    let response = commit(
        &fixture.cluster,
        &body("g", 1, "m1", &[("orders", &[0, 1])]),
        &open(),
    )
    .await;
    for partition in &response.topics[0].partitions {
        assert_eq!(partition.error_code, 0);
    }
    let group = oqueue_core::GroupId::new("g").expect("valid");
    let topic = TopicId::new("orders").expect("valid");
    assert_eq!(
        fixture.cluster.committed_offsets().get(&group, &topic, 0),
        Some(42)
    );
    assert_eq!(
        fixture.cluster.committed_offsets().get(&group, &topic, 1),
        Some(42)
    );
}

/// ⚠️ **Round-1 review's own finding, reproduced directly**: two commits to
/// the identical key can durably append in one order and then reach their
/// own post-append in-memory update in the *other* order — each runs in its
/// own spawned task (`connection.rs` gives every request its own), so
/// nothing ties the order two tasks resume in after their own `.await` to
/// the order their appends actually landed in the log. `super::apply` is
/// the guard: it must let the causally later version win even when it is
/// applied to the map *first*, not overwrite it with an earlier version
/// applied second — which is exactly the shape `commit`'s own doc names.
#[test]
fn a_later_committed_version_is_never_overwritten_by_an_earlier_one_applied_after_it() {
    use std::collections::HashMap;

    let group = oqueue_core::GroupId::new("g").expect("valid");
    let topic = TopicId::new("orders").expect("valid");
    let record_of = |offset: i64| GroupMetadataRecord::OffsetCommitted {
        group: group.clone(),
        topic: topic.clone(),
        partition: 0,
        offset,
    };

    let mut offsets = HashMap::new();
    // The later append (version 2, offset 20) reaches `apply` first --
    // the exact interleaving a losing scheduling race can produce.
    super::apply(&mut offsets, CommitVersion::new(2), &record_of(20));
    super::apply(&mut offsets, CommitVersion::new(1), &record_of(10));

    assert_eq!(
        offsets.get(&(group, topic, 0)),
        Some(&(CommitVersion::new(2), 20)),
        "the durably-later commit (version 2) must not be clobbered by the \
         durably-earlier one (version 1) arriving second"
    );
}

/// ⚠️ **`M4.12`'s own acceptance criterion, verbatim**: a commit from
/// principal A cannot land under a group session authenticated as
/// principal B -- `M9.7`'s own authorization decision point, reused here
/// rather than a second one invented. Batched: alice's own request names
/// both her own topic and bob's in one call, so this also proves one
/// unauthorized topic does not refuse the other, `M9.12`'s own
/// per-topic-not-per-request precedent.
#[tokio::test(start_paused = true)]
async fn a_commit_from_principal_a_cannot_land_under_principal_bs_topic() {
    let fixture = fixture(&["alice-topic", "bob-topic"]).await;
    seat(&fixture.cluster, "g", "m1");

    let mut grants = TopicGrants::new();
    grants.grant(
        Principal::new("alice").expect("valid"),
        TopicId::new("alice-topic").expect("valid"),
    );
    grants.grant(
        Principal::new("bob").expect("valid"),
        TopicId::new("bob-topic").expect("valid"),
    );
    let alice = Principal::new("alice").expect("valid");
    let authz = AuthzContext {
        principal: Some(&alice),
        credentials_configured: true,
        topic_grants: &grants,
    };

    let response = commit(
        &fixture.cluster,
        &body("g", 1, "m1", &[("alice-topic", &[0]), ("bob-topic", &[0])]),
        &authz,
    )
    .await;
    assert_response_lands_alice_refuses_bob(&response);
    assert_store_lands_alice_refuses_bob(&fixture.cluster);
}

/// Split out of the test above purely to keep it under
/// `code-structure.md`'s fifty-line limit — same acceptance criterion,
/// the response half.
fn assert_response_lands_alice_refuses_bob(response: &KpResponse) {
    assert_eq!(response.topics.len(), 2);
    let alice_topic = response
        .topics
        .iter()
        .find(|t| t.name.as_str() == "alice-topic")
        .expect("named");
    assert_eq!(
        alice_topic.partitions[0].error_code, 0,
        "alice's own topic must land"
    );
    let bob_topic = response
        .topics
        .iter()
        .find(|t| t.name.as_str() == "bob-topic")
        .expect("named");
    assert_eq!(
        bob_topic.partitions[0].error_code,
        oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        "bob's own topic must be refused"
    );
}

/// The store half of the same acceptance criterion.
fn assert_store_lands_alice_refuses_bob(cluster: &crate::cluster::Cluster) {
    let group = oqueue_core::GroupId::new("g").expect("valid");
    assert_eq!(
        cluster
            .committed_offsets()
            .get(&group, &TopicId::new("alice-topic").expect("valid"), 0),
        Some(42),
        "alice's own commit landed"
    );
    assert_eq!(
        cluster
            .committed_offsets()
            .get(&group, &TopicId::new("bob-topic").expect("valid"), 0),
        None,
        "the refused commit never landed"
    );
}

/// ⚠️ **`M4.14`'s own first acceptance criterion**: killing the
/// coordinator after a commit and before an ack is impossible by
/// construction — `ADR-0020`'s own "the ack IS the durable write" instinct
/// applied here. A refused durable append leaves nothing acknowledged as a
/// success and nothing landed in the in-memory map; healed, the identical
/// retry lands normally.
#[tokio::test(start_paused = true)]
async fn a_refused_durable_append_leaves_nothing_committed_and_no_success_ack() {
    let fixture = fixture(&["orders"]).await;
    seat(&fixture.cluster, "g", "m1");
    fixture.refuse_group_metadata_append();

    let response = commit(
        &fixture.cluster,
        &body("g", 1, "m1", &[("orders", &[0])]),
        &open(),
    )
    .await;
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::UNKNOWN_SERVER_ERROR,
        "a refused durable append must not be acknowledged as a success"
    );
    let group = oqueue_core::GroupId::new("g").expect("valid");
    let topic = TopicId::new("orders").expect("valid");
    assert_eq!(
        fixture.cluster.committed_offsets().get(&group, &topic, 0),
        None,
        "nothing lands in memory when the durable append is refused"
    );

    fixture.heal_group_metadata_log();
    let response = commit(
        &fixture.cluster,
        &body("g", 1, "m1", &[("orders", &[0])]),
        &open(),
    )
    .await;
    assert_eq!(
        response.topics[0].partitions[0].error_code, 0,
        "healed, the identical retry lands normally"
    );
    assert_eq!(
        fixture.cluster.committed_offsets().get(&group, &topic, 0),
        Some(42)
    );
}

/// ⚠️ **`M4.14`'s own other acceptance criterion**: every previously
/// committed offset replays intact from the same log — the restart this
/// milestone's own `FakeGroupMetadataLog` cannot itself survive
/// (`ADR-0035`) is stood in for here by opening a *second*
/// `CommittedOffsets` from the identical `Arc<dyn GroupMetadataLog>`, the
/// same "same log, fresh in-memory state" shape an actual process restart
/// would present.
#[tokio::test(start_paused = true)]
async fn every_committed_offset_replays_after_a_restart() {
    let fixture = fixture(&["orders", "payments"]).await;
    seat(&fixture.cluster, "g", "m1");
    commit(
        &fixture.cluster,
        &body("g", 1, "m1", &[("orders", &[0, 1]), ("payments", &[0])]),
        &open(),
    )
    .await;

    let log = Arc::clone(&fixture.group_metadata_log) as Arc<dyn GroupMetadataLog>;
    let replayed = super::CommittedOffsets::open(log)
        .await
        .expect("the log replays cleanly");

    let group = oqueue_core::GroupId::new("g").expect("valid");
    let orders = TopicId::new("orders").expect("valid");
    let payments = TopicId::new("payments").expect("valid");
    assert_eq!(replayed.get(&group, &orders, 0), Some(42));
    assert_eq!(replayed.get(&group, &orders, 1), Some(42));
    assert_eq!(replayed.get(&group, &payments, 0), Some(42));
}

/// `CommittedOffsets::open`'s own replay loop pages through the log
/// (`GroupMetadataLog`'s own guarantee 5: a short page means no more, a
/// full page means keep reading) rather than assuming one `read_from` call
/// sees everything — this plants one entry more than
/// [`super::REPLAY_PAGE_SIZE`] holds, directly against a bare log rather
/// than through hundreds of real `OffsetCommit` calls.
#[tokio::test(start_paused = true)]
async fn replay_pages_through_more_entries_than_one_page_holds() {
    let log = Arc::new(oqueue_core::FakeGroupMetadataLog::new());
    let group = oqueue_core::GroupId::new("g").expect("valid");
    let topic = TopicId::new("orders").expect("valid");
    let page_size = i32::try_from(super::REPLAY_PAGE_SIZE).expect("fits");
    let entries: Vec<GroupMetadataEntry> = (0..=page_size)
        .map(|partition| {
            GroupMetadataEntry::new(
                CommitVersion::new(u64::try_from(partition).expect("non-negative")),
                GroupMetadataRecord::OffsetCommitted {
                    group: group.clone(),
                    topic: topic.clone(),
                    partition,
                    offset: i64::from(partition),
                },
            )
        })
        .collect();
    log.append(&entries).await.expect("appends");

    let replayed = super::CommittedOffsets::open(Arc::clone(&log) as Arc<dyn GroupMetadataLog>)
        .await
        .expect("replays every page");
    assert_eq!(replayed.get(&group, &topic, 0), Some(0));
    assert_eq!(
        replayed.get(&group, &topic, page_size),
        Some(i64::from(page_size)),
        "the entry past the first page must not be lost"
    );
    // A short (non-empty) second page is itself the signal that the log has
    // no more — `GroupMetadataLog` guarantee 5. A reader that does not
    // trust that and pages once more anyway issues a wasted read_from call
    // against what a real engine bills per request; this pins the replay to
    // exactly the two calls the data requires (one full page, one short
    // one), not three.
    assert_eq!(
        log.read_from_calls(),
        2,
        "a short page must stop the replay at once, without an extra read past it"
    );
}

/// The boundary case the test above does not cover: a log holding *exactly*
/// one page's worth of entries. The single `read_from` call returns a full
/// page, so `CommittedOffsets::open` cannot yet tell from that call alone
/// whether more remains — it must issue a second, which then comes back
/// empty. Two calls either way, but for a different reason than the
/// short-second-page case above; both are asserted so a change that makes
/// either boundary wrong is caught by the boundary it actually breaks.
#[tokio::test(start_paused = true)]
async fn replay_stops_after_a_trailing_empty_page_when_the_log_ends_on_a_page_boundary() {
    let log = Arc::new(oqueue_core::FakeGroupMetadataLog::new());
    let group = oqueue_core::GroupId::new("g").expect("valid");
    let topic = TopicId::new("orders").expect("valid");
    let page_size = i32::try_from(super::REPLAY_PAGE_SIZE).expect("fits");
    let entries: Vec<GroupMetadataEntry> = (0..page_size)
        .map(|partition| {
            GroupMetadataEntry::new(
                CommitVersion::new(u64::try_from(partition).expect("non-negative")),
                GroupMetadataRecord::OffsetCommitted {
                    group: group.clone(),
                    topic: topic.clone(),
                    partition,
                    offset: i64::from(partition),
                },
            )
        })
        .collect();
    log.append(&entries).await.expect("appends");

    let replayed = super::CommittedOffsets::open(Arc::clone(&log) as Arc<dyn GroupMetadataLog>)
        .await
        .expect("replays the whole page");
    assert_eq!(
        replayed.get(&group, &topic, page_size - 1),
        Some(i64::from(page_size - 1))
    );
    assert_eq!(
        log.read_from_calls(),
        2,
        "a log ending exactly on a page boundary needs one more, empty read to know it ended"
    );
}

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(&fixture.cluster, prelude(), &[0xFF; 3], &open()).await;
    assert!(matches!(response, HandlerResponse::Close));
}

mod restart;
