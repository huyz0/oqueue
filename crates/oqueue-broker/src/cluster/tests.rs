#![allow(clippy::expect_used)]

use crate::cluster::{Cluster, Seams, Sequencing};
use crate::writer_id::WriterId;
use oqueue_coordinator::Coordinator;
use oqueue_core::{CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog, GroupMetadataLog};
use std::sync::Arc;

/// Credentials off: `offset_fetch` fails open — `offset_fetch/tests.rs`'s
/// own precedent for the same fixture.
static EMPTY_GRANTS: std::sync::LazyLock<oqueue_core::TopicGrants> =
    std::sync::LazyLock::new(oqueue_core::TopicGrants::new);

fn group(name: &str) -> oqueue_core::GroupId {
    oqueue_core::GroupId::new(name).expect("valid")
}

/// A fresh, real `Cluster` over empty fakes — **not** waited on, unlike
/// `crate::testing::fixture`, which since `M4.15a` calls
/// `Cluster::new(...).wait_until_replayed()` specifically so every other
/// handler test does not have to think about the window this one exists to
/// prove. Duplicated here rather than reusing `crate::testing`, because
/// this test's whole point is what happens *before* that wait.
async fn cluster_still_loading() -> Cluster {
    cluster_still_loading_over(Arc::new(oqueue_core::FakeGroupMetadataLog::new())).await
}

/// The same, over `group_metadata_log` (not built fresh here) — the split
/// half [`cluster_still_loading`] uses for the common empty-log case, and
/// a caller that wants to seed the group log ahead of time (a replay
/// failure test, say) uses directly.
async fn cluster_still_loading_over(group_metadata_log: Arc<dyn GroupMetadataLog>) -> Cluster {
    let log = Arc::new(FakeMetadataLog::new());
    let index = Box::new(FakeMaterializedIndex::new());
    let (coordinator, _serving, reader) = Coordinator::open(
        log,
        index,
        CoordinatorEpoch::new(1),
        Arc::new(oqueue_core::FakeClock::new()),
    )
    .await
    .expect("an empty log opens");
    Cluster::new(
        "h",
        1,
        Sequencing::new(coordinator, reader),
        Seams {
            store: Arc::new(oqueue_core::FakeObjectStore::new()),
            group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
            group_metadata_log,
        },
        &WriterId::mint(),
    )
    .await
    .expect("a minted identity is a usable key component, and an empty log opens")
}

/// A fresh `Cluster` over `group_metadata_log` (shared, not built fresh
/// here) and a *fresh* `FakeGroupCoordinator` — `M4.15c`'s own restart
/// simulation: the durable log survives, the in-memory coordinator does
/// not, the same shape `M4.14`'s own restart test already established for
/// offsets. Waited on before returning, unlike [`cluster_still_loading`]:
/// this helper is for tests proving what *does* survive, not the window
/// before it does.
async fn cluster_over(group_metadata_log: Arc<dyn GroupMetadataLog>) -> Cluster {
    let log = Arc::new(FakeMetadataLog::new());
    let index = Box::new(FakeMaterializedIndex::new());
    let (coordinator, _serving, reader) = Coordinator::open(
        log,
        index,
        CoordinatorEpoch::new(1),
        Arc::new(oqueue_core::FakeClock::new()),
    )
    .await
    .expect("an empty log opens");
    let cluster = Cluster::new(
        "h",
        1,
        Sequencing::new(coordinator, reader),
        Seams {
            store: Arc::new(oqueue_core::FakeObjectStore::new()),
            group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
            group_metadata_log,
        },
        &WriterId::mint(),
    )
    .await
    .expect("a minted identity is a usable key component, and an empty log opens");
    cluster.wait_until_replayed().await;
    cluster
}

/// `M4.15c`'s own acceptance criterion, verbatim: a group's state
/// (membership, generation) after replay matches what it was before a
/// restart, across an interleaved multi-group sequence of heartbeat
/// evictions, batched leaves, and syncs.
///
/// ⚠️ **Every event, including `Join`, goes through
/// `group_transitions().transition(...)` directly, not through the real
/// wire handlers.** What this row built is the durability mechanism those
/// handlers now call into (`sync_group.rs`'s own real call, and
/// `heartbeat.rs`'s `remove_where`, which this test's own direct calls
/// stand in for) — the mechanism is what needs proving here, and a wire
/// round-trip would only add encoding noise a real handler test already
/// covers elsewhere. ⚠️ This test routes its own `Join` events through the
/// actor rather than through a real `JoinGroup` request — which it could do
/// even when `join_group::round.rs` still called
/// `GroupCoordinator::transition` directly, because the actor does not care
/// who the caller is. `M4.15d` since retrofitted that call site, so the
/// shortcut now matches production rather than standing in for it.
/// Seeding `Join` any other
/// way here would leave it out of the durable log, and replay would then
/// fail outright the moment it reached a later `JoinBarrierComplete` with
/// no matching `Join` behind it.
#[tokio::test(start_paused = true)]
async fn group_state_survives_a_restart_across_interleaved_multi_group_transitions() {
    let log: Arc<dyn GroupMetadataLog> = Arc::new(oqueue_core::FakeGroupMetadataLog::new());
    let before = cluster_over(Arc::clone(&log)).await;
    let orders = group("orders");
    let payments = group("payments");

    seed_and_interleave(&before, &orders, &payments).await;

    let orders_before = before
        .group_coordinator()
        .record(&orders)
        .expect("orders has a record");
    let payments_before = before
        .group_coordinator()
        .record(&payments)
        .expect("payments has a record");
    assert_eq!(orders_before.state, oqueue_core::GroupState::Stable);
    assert_eq!(payments_before.state, oqueue_core::GroupState::Stable);

    // The "restart": a fresh Cluster, fresh in-memory GroupCoordinator,
    // same durable log.
    let after = cluster_over(log).await;
    let orders_after = after
        .group_coordinator()
        .record(&orders)
        .expect("orders replayed into existence");
    let payments_after = after
        .group_coordinator()
        .record(&payments)
        .expect("payments replayed into existence");

    assert_eq!(
        orders_after, orders_before,
        "orders' own state must survive the restart"
    );
    assert_eq!(
        payments_after, payments_before,
        "payments' own state must survive the restart"
    );
}

/// Fires `event` against `group` through the actor, panicking (naming
/// both) if it is refused — every event in
/// [`seed_and_interleave`]'s own sequence is expected to be legal.
async fn fire(cluster: &Cluster, group: &oqueue_core::GroupId, event: oqueue_core::GroupEvent) {
    cluster
        .group_transitions()
        .transition(group.clone(), event)
        .await
        .unwrap_or_else(|e| panic!("{event:?} against {group:?} must be legal here: {e}"));
}

/// Seeds `orders` and `payments` to `Stable`, then drives each through its
/// own eviction-then-rejoin-then-resync sequence, deliberately out of
/// lockstep with each other, through the same shared actor — split out of
/// the test itself purely for `code-structure.md`'s own fifty-line limit.
async fn seed_and_interleave(
    cluster: &Cluster,
    orders: &oqueue_core::GroupId,
    payments: &oqueue_core::GroupId,
) {
    use oqueue_core::GroupEvent::{AllMembersGone, Join, JoinBarrierComplete, SyncComplete};

    for g in [orders, payments] {
        fire(cluster, g, Join).await;
        fire(cluster, g, JoinBarrierComplete).await;
        fire(cluster, g, SyncComplete).await;
    }

    fire(cluster, orders, AllMembersGone).await;
    fire(cluster, payments, AllMembersGone).await;
    fire(cluster, orders, Join).await;
    fire(cluster, orders, JoinBarrierComplete).await;
    fire(cluster, payments, Join).await;
    fire(cluster, orders, SyncComplete).await;
    fire(cluster, payments, JoinBarrierComplete).await;
    fire(cluster, payments, SyncComplete).await;
}

/// `M4.15a`'s own acceptance criterion, verbatim: a request driven against
/// a coordinator mid-replay gets `COORDINATOR_LOAD_IN_PROGRESS`, not a
/// silently wrong answer — proven with `Heartbeat`, called immediately
/// after `Cluster::new` returns, before anything has a chance to yield to
/// the background replay task `tokio::spawn` never runs inline with its
/// own spawner. `Heartbeat` became `async` in `M4.15c`, but this call
/// still never yields: `group` was never joined, so `sweep`'s own
/// `remove_where` finds nothing to remove and returns before touching the
/// actor at all — the same "resolves in one poll" reasoning `Cluster::new`
/// itself relies on. Then, once the same cluster reports ready, the
/// identical call is answered on its *other* merits (`UNKNOWN_MEMBER_ID`,
/// since nothing ever joined) rather than staying stuck on the
/// load-in-progress code — proving the gate actually closes, not just
/// that it can be observed open once.
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

    let during = heartbeat(&cluster, "g", "m1", 1).await;
    assert_eq!(
        during,
        oqueue_codec::error_codes::COORDINATOR_LOAD_IN_PROGRESS,
        "a request arriving mid-replay must get the loading code, not \
         UNKNOWN_MEMBER_ID or any other answer racing ahead of replay"
    );

    cluster.wait_until_replayed().await;
    assert!(!cluster.replay_in_progress());

    let after = heartbeat(&cluster, "g", "m1", 1).await;
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

/// `M4.15c` round-1 review's own finding: once replay succeeds, the same
/// background task falls through into
/// `crate::group_transitions::GroupTransitionsTask::serve` and keeps
/// running for as long as `Cluster` lives — `group_transitions_task_alive`
/// is what lets a caller confirm that, satisfying `async-concurrency.md`
/// rule 13's "an owner that can observe" bar for the serving phase, not
/// only the replay phase `wait_until_replayed` already covers.
#[tokio::test(start_paused = true)]
async fn the_background_task_stays_alive_through_serving_after_replay() {
    let cluster = cluster_still_loading().await;
    cluster.wait_until_replayed().await;
    assert!(
        cluster.group_transitions_task_alive(),
        "the same task that just finished replay must still be running its own serve loop"
    );
}

/// The other half of the same claim: `group_transitions_task_alive` must
/// genuinely report `false` when the background task has actually ended,
/// not just `true` unconditionally — proven by making replay itself fail
/// outright (a real `CommitVersionOverflow`, not the per-group-poisoned
/// `IllegalGroupTransition` path `M4.15c`'s own round-1 review already
/// made non-fatal), so the task ends before ever reaching `serve`.
#[tokio::test(start_paused = true)]
async fn the_background_task_is_not_alive_once_replay_itself_fails_outright() {
    // `crate::group_transitions::REPLAY_PAGE_SIZE`'s own value — not
    // referenced directly (private to a module this one is not a
    // descendant of); a full page is what makes replay's own loop reach
    // `last.version().advance(1)` at all, `check-drift.sh`'s own pin.
    let page_size: u64 = 256;
    let start = u64::MAX - (page_size - 1);
    let log = Arc::new(oqueue_core::FakeGroupMetadataLog::new());
    let entries: Vec<oqueue_core::GroupMetadataEntry> = (0..page_size)
        .map(|i| {
            oqueue_core::GroupMetadataEntry::new(
                oqueue_core::CommitVersion::new(start + i),
                oqueue_core::GroupMetadataRecord::GroupTransitioned {
                    group: group(&format!("g{i}")),
                    event: oqueue_core::GroupEvent::Join,
                },
            )
        })
        .collect();
    log.append(&entries).await.expect("appends");

    let cluster = cluster_still_loading_over(log).await;
    // Give the background task a real chance to run replay to its own
    // failure and end — `yield_now` in a loop, not a timer: this test
    // does not depend on the paused clock advancing at all.
    for _ in 0..1000 {
        if !cluster.group_transitions_task_alive() {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert!(
        !cluster.group_transitions_task_alive(),
        "the task must have ended once GroupTransitionsTask::replay's own \
         CommitVersion::advance overflowed"
    );
    assert!(
        cluster.replay_in_progress(),
        "the gate must never open when replay itself failed outright"
    );
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

async fn heartbeat(cluster: &Cluster, group: &str, member_id: &str, generation: i32) -> i16 {
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
        crate::heartbeat::handle(cluster, prelude(12, HEARTBEAT_VERSION), &body).await
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

mod catalog;
mod degraded_replay;
