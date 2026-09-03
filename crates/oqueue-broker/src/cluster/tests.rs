#![allow(clippy::expect_used)]

use crate::cluster::{Cluster, Seams, Sequencing};
use crate::writer_id::WriterId;
use oqueue_coordinator::Coordinator;
use oqueue_core::{CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog};
use std::sync::Arc;

/// Credentials off: `offset_fetch` fails open — `offset_fetch/tests.rs`'s
/// own precedent for the same fixture.
static EMPTY_GRANTS: std::sync::LazyLock<oqueue_core::TopicGrants> =
    std::sync::LazyLock::new(oqueue_core::TopicGrants::new);

/// A fresh, real `Cluster` over empty fakes — **not** waited on, unlike
/// `crate::testing::fixture`, which since `M4.15a` calls
/// `Cluster::new(...).wait_until_replayed()` specifically so every other
/// handler test does not have to think about the window this one exists to
/// prove. Duplicated here rather than reusing `crate::testing`, because
/// this test's whole point is what happens *before* that wait.
async fn cluster_still_loading() -> Cluster {
    let log = Arc::new(FakeMetadataLog::new());
    let index = Box::new(FakeMaterializedIndex::new());
    let (coordinator, _serving, reader) = Coordinator::open(log, index, CoordinatorEpoch::new(1))
        .await
        .expect("an empty log opens");
    Cluster::new(
        "h",
        1,
        Sequencing::new(coordinator, reader),
        Seams {
            store: Arc::new(oqueue_core::FakeObjectStore::new()),
            group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
            group_metadata_log: Arc::new(oqueue_core::FakeGroupMetadataLog::new()),
        },
        &WriterId::mint(),
    )
    .await
    .expect("a minted identity is a usable key component, and an empty log opens")
}

/// `M4.15a`'s own acceptance criterion, verbatim: a request driven against
/// a coordinator mid-replay gets `COORDINATOR_LOAD_IN_PROGRESS`, not a
/// silently wrong answer — proven with `Heartbeat` (a plain, synchronous
/// handler, `heartbeat.rs`'s own module doc) called immediately after
/// `Cluster::new` returns, before anything has a chance to yield to the
/// background replay task `tokio::spawn` never runs inline with its own
/// spawner. Then, once the same cluster reports ready, the identical call
/// is answered on its *other* merits (`UNKNOWN_MEMBER_ID`, since nothing
/// ever joined) rather than staying stuck on the load-in-progress code —
/// proving the gate actually closes, not just that it can be observed
/// open once.
///
/// ⚠️ **`flavor = "current_thread"` named explicitly, not left as the
/// unstated default.** The "nothing has a chance to yield" claim above is
/// only true on a single-thread runtime — `tests/it/connection.rs` and
/// `dispatch/tests.rs` both use `flavor = "multi_thread"` elsewhere in
/// this crate, and on one, the spawned replay task could run concurrently
/// on a second worker thread, racing `mark_ready()` against the assertion
/// right after `cluster_still_loading()` returns. Naming the flavor here
/// means a future edit that changes it is a visible, deliberate diff
/// rather than a silent flake introduced by copying this test elsewhere.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_request_mid_replay_is_refused_coordinator_load_in_progress() {
    let cluster = cluster_still_loading().await;
    assert!(
        cluster.replay_in_progress(),
        "a freshly-returned Cluster must not have replayed yet -- \
         tokio::spawn never runs its task inline with the spawner"
    );

    let during = heartbeat(&cluster, "g", "m1", 1);
    assert_eq!(
        during,
        oqueue_codec::error_codes::COORDINATOR_LOAD_IN_PROGRESS,
        "a request arriving mid-replay must get the loading code, not \
         UNKNOWN_MEMBER_ID or any other answer racing ahead of replay"
    );

    cluster.wait_until_replayed().await;
    assert!(!cluster.replay_in_progress());

    let after = heartbeat(&cluster, "g", "m1", 1);
    assert_eq!(
        after,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID,
        "once replay finishes the same request must be judged on its own \
         merits again, not stay stuck answering the loading code forever"
    );
}

/// One call, one answer: `OffsetFetch` has no `crate::fencing` seam of its
/// own (`offset_fetch.rs`'s own module doc), so it needs its own direct
/// check — this is the request most at risk of the silently-wrong-answer
/// shape `behavior.md` rule 11 forbids, since without this check it would
/// read an empty, not-yet-replayed map and honestly (and wrongly) report
/// "never committed" for a group that has.
///
/// ⚠️ `flavor = "current_thread"` named explicitly — same reason as the
/// test above.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn offset_fetch_mid_replay_is_also_refused_coordinator_load_in_progress() {
    let cluster = cluster_still_loading().await;
    assert!(cluster.replay_in_progress());

    let error_code = offset_fetch(&cluster, "g");
    assert_eq!(
        error_code,
        oqueue_codec::error_codes::COORDINATOR_LOAD_IN_PROGRESS
    );

    cluster.wait_until_replayed().await;
    assert_eq!(offset_fetch(&cluster, "g"), oqueue_codec::error_codes::NONE);
}

const HEARTBEAT_VERSION: i16 = 2;
const OFFSET_FETCH_VERSION: i16 = 7;

fn prelude(api_key: i16, version: i16) -> oqueue_codec::frame::RequestPrelude {
    oqueue_codec::frame::RequestPrelude {
        api_key,
        api_version: version,
        correlation_id: 1,
    }
}

fn heartbeat(cluster: &Cluster, group: &str, member_id: &str, generation: i32) -> i16 {
    use kafka_protocol::messages::HeartbeatRequest as KpRequest;
    use kafka_protocol::messages::HeartbeatResponse as KpResponse;
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};

    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(generation)
        .with_member_id(StrBytes::from_string(member_id.to_owned()));
    let mut body = Vec::new();
    request
        .encode(&mut body, HEARTBEAT_VERSION)
        .expect("encodes");

    let crate::connection::HandlerResponse::Reply(out) =
        crate::heartbeat::handle(cluster, prelude(12, HEARTBEAT_VERSION), &body)
    else {
        panic!("a Heartbeat replies");
    };
    let mut rest = &out[4..]; // v2 is not flexible: a 4-byte header.
    KpResponse::decode(&mut rest, HEARTBEAT_VERSION)
        .expect("decodes")
        .error_code
}

/// The top-level `error_code` of an all-topics `OffsetFetch` for `group` —
/// the only field this test needs, `offset_fetch.rs`'s own module doc: a
/// request with no fencing of its own answers `COORDINATOR_LOAD_IN_PROGRESS`
/// there, not per topic/partition.
fn offset_fetch(cluster: &Cluster, group: &str) -> i16 {
    use kafka_protocol::messages::OffsetFetchRequest as KpRequest;
    use kafka_protocol::messages::OffsetFetchResponse as KpResponse;
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};

    let request = KpRequest::default().with_group_id(kafka_protocol::messages::GroupId(
        StrBytes::from_string(group.to_owned()),
    ));
    let mut body = Vec::new();
    request
        .encode(&mut body, OFFSET_FETCH_VERSION)
        .expect("encodes");

    let authz = crate::authz::AuthzContext {
        principal: None,
        credentials_configured: false,
        topic_grants: &EMPTY_GRANTS,
    };
    let crate::connection::HandlerResponse::Reply(out) =
        crate::offset_fetch::handle(cluster, prelude(9, OFFSET_FETCH_VERSION), &body, &authz)
    else {
        panic!("an OffsetFetch replies");
    };
    let mut rest = &out[5..]; // v7 is flexible: a 5-byte response header.
    KpResponse::decode(&mut rest, OFFSET_FETCH_VERSION)
        .expect("decodes")
        .error_code
}
