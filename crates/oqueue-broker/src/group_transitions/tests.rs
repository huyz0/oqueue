#![allow(clippy::expect_used)]

use super::{GroupTransitions, append_durably, handle_one};
use oqueue_core::{
    CommitVersion, FakeGroupCoordinator, FakeGroupMetadataLog, FaultGroupMetadataLog,
    GroupCoordinator, GroupEvent, GroupId, GroupMetadataEntry, GroupMetadataLog,
    GroupMetadataRecord, GroupState,
};

fn group(name: &str) -> GroupId {
    GroupId::new(name).expect("valid")
}

/// A legal transition is applied to the live coordinator and durably
/// appended — the acceptance criterion `handle_one`'s own doc states,
/// exercised directly rather than only through the full actor.
#[tokio::test(start_paused = true)]
async fn a_legal_transition_is_applied_and_durably_appended() {
    let coordinator = FakeGroupCoordinator::new();
    let log = FakeGroupMetadataLog::new();
    let g = group("orders");

    let record = handle_one(&coordinator, &log, &g, GroupEvent::Join)
        .await
        .expect("Empty -> Join is legal");
    assert_eq!(record.state, GroupState::PreparingRebalance);
    assert_eq!(
        coordinator.record(&g).map(|r| r.state),
        Some(GroupState::PreparingRebalance)
    );
    assert_eq!(log.len(), 1, "the transition must be durably appended");
}

/// A refused transition touches neither the live coordinator nor the
/// durable log — `handle_one`'s own doc: validated *before* it is
/// durable, never after.
#[tokio::test(start_paused = true)]
async fn an_illegal_transition_touches_neither_the_coordinator_nor_the_log() {
    let coordinator = FakeGroupCoordinator::new();
    let log = FakeGroupMetadataLog::new();
    let g = group("orders");

    // Empty -> SyncComplete has no legal arm (`GroupState::transition`'s
    // own table).
    let result = handle_one(&coordinator, &log, &g, GroupEvent::SyncComplete).await;
    assert!(result.is_err());
    assert_eq!(
        coordinator.record(&g),
        None,
        "an illegal event must not create a record"
    );
    assert_eq!(
        log.len(),
        0,
        "an illegal event must not be durably appended"
    );
}

/// A durable-append failure leaves the live coordinator exactly as it
/// was — `ADR-0020`'s own "the ack IS the durable write" instinct,
/// `M4.14`'s fault-injection precedent applied here.
#[tokio::test(start_paused = true)]
async fn a_refused_durable_append_leaves_the_live_coordinator_untouched() {
    let coordinator = FakeGroupCoordinator::new();
    let log = FaultGroupMetadataLog::new(FakeGroupMetadataLog::new());
    log.refuse_append();
    let g = group("orders");

    let result = handle_one(&coordinator, &log, &g, GroupEvent::Join).await;
    assert!(
        result.is_err(),
        "a durable-append failure must not be reported as a success"
    );
    assert_eq!(
        coordinator.record(&g),
        None,
        "the live coordinator must not move ahead of what the log could durably record"
    );
}

/// Two transitions for the same group, applied and appended in order —
/// the property the whole actor exists for: durable order matches
/// live-apply order, proven directly against `handle_one` called
/// sequentially (the same way `GroupTransitionsTask::serve`'s own
/// single-consumer loop calls it).
#[tokio::test(start_paused = true)]
async fn sequential_transitions_land_in_the_log_in_the_same_order_applied() {
    let coordinator = FakeGroupCoordinator::new();
    let log = FakeGroupMetadataLog::new();
    let g = group("orders");

    handle_one(&coordinator, &log, &g, GroupEvent::Join)
        .await
        .expect("Empty -> Join is legal");
    handle_one(&coordinator, &log, &g, GroupEvent::JoinBarrierComplete)
        .await
        .expect("PreparingRebalance -> CompletingRebalance is legal");

    assert_eq!(log.len(), 2);
    let page = log.read_from(CommitVersion::ZERO, 10).await.expect("reads");
    let events: Vec<GroupEvent> = page
        .iter()
        .map(|entry| match entry.record() {
            GroupMetadataRecord::GroupTransitioned { event, .. } => *event,
            GroupMetadataRecord::OffsetCommitted { .. } => panic!("unexpected record"),
        })
        .collect();
    assert_eq!(
        events,
        vec![GroupEvent::Join, GroupEvent::JoinBarrierComplete],
        "the log's own order must match the order the two calls actually happened in"
    );
}

/// Replay re-fires every durable event, in order, reconstructing the
/// exact state a fresh coordinator never saw live — `GroupTransitionsTask::replay`'s
/// own acceptance criterion, proven directly against a hand-built log
/// rather than through a full `Cluster` restart.
#[tokio::test(start_paused = true)]
async fn replay_reconstructs_state_from_a_log_a_fresh_coordinator_never_saw_live() {
    let log = FakeGroupMetadataLog::new();
    let g = group("orders");
    let entries = vec![
        GroupMetadataEntry::new(
            CommitVersion::new(0),
            GroupMetadataRecord::GroupTransitioned {
                group: g.clone(),
                event: GroupEvent::Join,
            },
        ),
        GroupMetadataEntry::new(
            CommitVersion::new(1),
            GroupMetadataRecord::GroupTransitioned {
                group: g.clone(),
                event: GroupEvent::JoinBarrierComplete,
            },
        ),
        GroupMetadataEntry::new(
            CommitVersion::new(2),
            GroupMetadataRecord::GroupTransitioned {
                group: g.clone(),
                event: GroupEvent::SyncComplete,
            },
        ),
    ];
    log.append(&entries).await.expect("appends");

    let coordinator = FakeGroupCoordinator::new();
    let (_transitions, task) = GroupTransitions::new();
    task.replay(&coordinator, &log).await.expect("replays");

    let record = coordinator.record(&g).expect("replayed into existence");
    assert_eq!(record.state, GroupState::Stable);
    assert_eq!(record.generation.get(), 1);
}

/// `append_durably` retries a losing race against another writer to the
/// same log — `CommittedOffsets::commit`'s own precedent, proven
/// directly: seed the log with an entry at version 0 from *outside* this
/// call, then confirm `append_durably` still lands at version 1 rather
/// than failing.
#[tokio::test(start_paused = true)]
async fn append_durably_retries_past_a_version_already_taken() {
    let log = FakeGroupMetadataLog::new();
    let g = group("orders");
    let already_there = GroupMetadataEntry::new(
        CommitVersion::ZERO,
        GroupMetadataRecord::GroupTransitioned {
            group: g.clone(),
            event: GroupEvent::Join,
        },
    );
    log.append(std::slice::from_ref(&already_there))
        .await
        .expect("seeds version 0");

    append_durably(&log, &g, GroupEvent::JoinBarrierComplete)
        .await
        .expect("retries past the taken version and lands at 1");
    assert_eq!(log.len(), 2);
}

/// `replay`'s own paging loop reads exactly one more page than a full one
/// holds, not an extra (wasted, `read_from_calls`-visible) empty one past
/// the end — `CommittedOffsets::replay`'s own identical pagination-boundary
/// test, `M4.14`/`M4.15a`'s own precedent for exactly this mutation shape
/// (`got < REPLAY_PAGE_SIZE` mutated to `==`/`>`/`<=` all survived the
/// first version of this test suite, since three entries never touch the
/// boundary at all).
#[tokio::test(start_paused = true)]
async fn replay_pages_through_more_entries_than_one_page_holds() {
    let log = FakeGroupMetadataLog::new();
    let page_size = i32::try_from(super::REPLAY_PAGE_SIZE).expect("fits");
    // One `Join` per group, spread across `page_size + 1` distinct groups
    // — trivially legal (every group starts `Empty`), so this needs no
    // sequence-of-events-per-group bookkeeping to stay valid.
    let entries: Vec<GroupMetadataEntry> = (0..=page_size)
        .map(|i| {
            GroupMetadataEntry::new(
                CommitVersion::new(u64::try_from(i).expect("non-negative")),
                GroupMetadataRecord::GroupTransitioned {
                    group: group(&format!("g{i}")),
                    event: GroupEvent::Join,
                },
            )
        })
        .collect();
    log.append(&entries).await.expect("appends");

    let coordinator = FakeGroupCoordinator::new();
    let (_transitions, task) = GroupTransitions::new();
    task.replay(&coordinator, &log).await.expect("replays");

    assert_eq!(
        coordinator.record(&group("g0")).map(|r| r.state),
        Some(GroupState::PreparingRebalance)
    );
    assert_eq!(
        coordinator
            .record(&group(&format!("g{page_size}")))
            .map(|r| r.state),
        Some(GroupState::PreparingRebalance),
        "the entry past the first page must not be lost"
    );
    assert_eq!(
        log.read_from_calls(),
        2,
        "a short trailing page must stop the replay at once, without an extra read past it"
    );
}

/// Round-1 review's own finding, reproduced directly: an orphaned durable
/// record (a `SyncComplete` with no `Join` behind it — exactly what
/// `sync_group.rs`/`heartbeat.rs` durably log today, since
/// `join_group::round.rs` itself is not migrated until `M4.15d`) must not
/// abort replay for any *other* group. `orders` is orphaned this way;
/// `payments` has a fully legal sequence right behind it in the same log.
#[tokio::test(start_paused = true)]
async fn an_orphaned_transition_poisons_only_its_own_group() {
    let log = FakeGroupMetadataLog::new();
    let orders = group("orders");
    let payments = group("payments");
    let entries = vec![
        // `orders`: a `SyncComplete` with no `Join`/`JoinBarrierComplete`
        // behind it — illegal from `Empty`, the exact shape a real,
        // not-yet-fully-migrated deployment produces today.
        GroupMetadataEntry::new(
            CommitVersion::new(0),
            GroupMetadataRecord::GroupTransitioned {
                group: orders.clone(),
                event: GroupEvent::SyncComplete,
            },
        ),
        // `payments`: a complete, legal sequence.
        GroupMetadataEntry::new(
            CommitVersion::new(1),
            GroupMetadataRecord::GroupTransitioned {
                group: payments.clone(),
                event: GroupEvent::Join,
            },
        ),
        GroupMetadataEntry::new(
            CommitVersion::new(2),
            GroupMetadataRecord::GroupTransitioned {
                group: payments.clone(),
                event: GroupEvent::JoinBarrierComplete,
            },
        ),
        GroupMetadataEntry::new(
            CommitVersion::new(3),
            GroupMetadataRecord::GroupTransitioned {
                group: payments.clone(),
                event: GroupEvent::SyncComplete,
            },
        ),
    ];
    log.append(&entries).await.expect("appends");

    let coordinator = FakeGroupCoordinator::new();
    let (_transitions, task) = GroupTransitions::new();
    task.replay(&coordinator, &log)
        .await
        .expect("replay succeeds overall despite orders' own orphaned entry");

    assert_eq!(
        coordinator.record(&orders),
        None,
        "orders must never accrue a record from an entry it has no legal predecessor for"
    );
    assert_eq!(
        coordinator.record(&payments).map(|r| r.state),
        Some(GroupState::Stable),
        "payments' own legal sequence must replay correctly regardless of orders' own failure"
    );
}
