#![allow(clippy::expect_used)]

use super::{FencingContext, NodeReadiness, Refusal, fence};
use oqueue_core::{
    FakeGroupCoordinator, GroupCoordinator, GroupEvent, GroupId, GroupRecord, GroupState,
};

fn group(name: &str) -> GroupId {
    GroupId::new(name).expect("valid")
}

/// A record for a freshly-joined group — `PreparingRebalance`, generation
/// still `0` (`Join` never bumps it). ⚠️ **Built from a real
/// [`FakeGroupCoordinator`], not a hand-written literal** — `GenerationId`/
/// `AssignmentEpoch` have no public constructor outside `oqueue_core`
/// (`group_coordinator.rs`'s own "no setter" doc), the same reason every
/// handler test in this crate drives a real coordinator rather than
/// fabricating a `GroupRecord`.
fn preparing_record() -> GroupRecord {
    let c = FakeGroupCoordinator::new();
    c.transition(&group("orders"), GroupEvent::Join)
        .expect("Empty -> Join is legal")
}

/// A record for a group that has completed one round — `Stable`,
/// generation `1`.
fn stable_record() -> GroupRecord {
    let g = group("orders");
    let c = FakeGroupCoordinator::new();
    c.transition(&g, GroupEvent::Join)
        .expect("Empty -> Join is legal");
    c.transition(&g, GroupEvent::JoinBarrierComplete)
        .expect("PreparingRebalance -> CompletingRebalance is legal");
    c.transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal")
}

/// ⚠️ **`M4.11`'s own acceptance criterion, verbatim**: every one of the
/// five codes, against the exact state/input that must produce it. Two
/// (`NotCoordinator`, `CoordinatorNotAvailable`) have no real call site in
/// this milestone's own v1 architecture (`ADR-0033`, module doc) — the
/// input is constructed directly here rather than through
/// [`FencingContext::for_this_node`], the same "prove the code, not the
/// reachability" idiom `crates/oqueue-core/src/group_state.rs`'s own
/// `Dead`/`Expire` table cells use for a transition nothing today drives.
/// Split across several `#[test]` functions purely for `code-structure.md`'s
/// own fifty-line limit — one acceptance criterion, several assertions.
#[test]
fn load_in_progress_is_checked_first_regardless_of_anything_else() {
    let stable = stable_record();
    let ctx = FencingContext {
        node: NodeReadiness {
            is_coordinator: true,
            coordinator_available: true,
            load_in_progress: true,
        },
        member_tracked: true,
        record: Some(&stable),
        generation_id: Some(stable.generation.get()),
        acceptable_states: Some(&[GroupState::Stable]),
    };
    assert_eq!(fence(&ctx), Err(Refusal::CoordinatorLoadInProgress));
}

#[test]
fn not_hosting_the_group_at_all_is_not_coordinator() {
    let stable = stable_record();
    let ctx = FencingContext {
        node: NodeReadiness {
            is_coordinator: false,
            coordinator_available: true,
            load_in_progress: false,
        },
        member_tracked: true,
        record: Some(&stable),
        generation_id: Some(stable.generation.get()),
        acceptable_states: Some(&[GroupState::Stable]),
    };
    assert_eq!(fence(&ctx), Err(Refusal::NotCoordinator));
}

#[test]
fn hosting_but_not_ready_is_coordinator_not_available() {
    let stable = stable_record();
    let ctx = FencingContext {
        node: NodeReadiness {
            is_coordinator: true,
            coordinator_available: false,
            load_in_progress: false,
        },
        member_tracked: true,
        record: Some(&stable),
        generation_id: Some(stable.generation.get()),
        acceptable_states: Some(&[GroupState::Stable]),
    };
    assert_eq!(fence(&ctx), Err(Refusal::CoordinatorNotAvailable));
}

#[test]
fn never_tracked_at_all_is_unknown_member() {
    let stable = stable_record();
    let ctx = FencingContext::for_this_node(
        false,
        Some(&stable),
        Some(stable.generation.get()),
        Some(&[GroupState::Stable]),
    );
    assert_eq!(fence(&ctx), Err(Refusal::UnknownMember));
}

/// The defensive arm — tracked, but no record at all (never reachable
/// through a real call site, module doc's own note).
#[test]
fn tracked_with_no_record_at_all_is_also_unknown_member() {
    let ctx = FencingContext::for_this_node(true, None, None, None);
    assert_eq!(fence(&ctx), Err(Refusal::UnknownMember));
}

#[test]
fn tracked_but_a_stale_generation_is_illegal_generation() {
    let stable = stable_record();
    let ctx = FencingContext::for_this_node(
        true,
        Some(&stable),
        Some(stable.generation.get() + 1),
        Some(&[GroupState::Stable]),
    );
    assert_eq!(fence(&ctx), Err(Refusal::IllegalGeneration));
}

#[test]
fn tracked_current_generation_but_the_wrong_state_is_rebalance_in_progress() {
    let preparing = preparing_record();
    let ctx = FencingContext::for_this_node(
        true,
        Some(&preparing),
        Some(preparing.generation.get()),
        Some(&[GroupState::Stable]),
    );
    assert_eq!(fence(&ctx), Err(Refusal::RebalanceInProgress));
}

#[test]
fn tracked_current_generation_acceptable_state_is_ok() {
    let stable = stable_record();
    let ctx = FencingContext::for_this_node(
        true,
        Some(&stable),
        Some(stable.generation.get()),
        Some(&[GroupState::Stable]),
    );
    assert_eq!(fence(&ctx), Ok(()));
}

/// A request whose own wire shape carries no generation (`JoinGroup`,
/// `LeaveGroup`) skips that check entirely rather than refusing on a
/// fabricated mismatch.
#[test]
fn no_generation_field_on_the_wire_means_no_generation_check() {
    let preparing = preparing_record();
    let ctx = FencingContext::for_this_node(true, Some(&preparing), None, None);
    assert_eq!(
        fence(&ctx),
        Ok(()),
        "no generation_id and no state requirement -- only membership is checked"
    );
}

/// A request with no state requirement at all (`LeaveGroup`'s own case: a
/// member may leave from any live state) is not refused
/// `RebalanceInProgress` for being, say, `PreparingRebalance`.
#[test]
fn no_state_requirement_accepts_any_state() {
    for r in [preparing_record(), stable_record()] {
        let ctx = FencingContext::for_this_node(true, Some(&r), None, None);
        assert_eq!(fence(&ctx), Ok(()), "{:?} must be accepted", r.state);
    }
}

/// Every [`Refusal`] variant answers the wire code `error_codes.rs`'s own
/// differential test already pins against the dependency -- this test
/// binds `Refusal` to those constants, not to the dependency directly, so
/// a mismatch is caught here rather than only downstream in a handler.
#[test]
fn every_refusal_names_its_own_pinned_wire_code() {
    assert_eq!(
        Refusal::CoordinatorLoadInProgress.error_code(),
        oqueue_codec::error_codes::COORDINATOR_LOAD_IN_PROGRESS
    );
    assert_eq!(
        Refusal::NotCoordinator.error_code(),
        oqueue_codec::error_codes::NOT_COORDINATOR
    );
    assert_eq!(
        Refusal::CoordinatorNotAvailable.error_code(),
        oqueue_codec::error_codes::COORDINATOR_NOT_AVAILABLE
    );
    assert_eq!(
        Refusal::UnknownMember.error_code(),
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
    assert_eq!(
        Refusal::IllegalGeneration.error_code(),
        oqueue_codec::error_codes::ILLEGAL_GENERATION
    );
    assert_eq!(
        Refusal::RebalanceInProgress.error_code(),
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS
    );
}
