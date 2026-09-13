//! `GroupState`'s own transition table, exhaustively.
//!
//! ⚠️ **Its own file because `group_state.rs` reached `code-structure.md`
//! rule 16's 500-line limit** when `M4.34` added `GroupEvent::MemberLeft`
//! and its three table entries. Split along the `#[cfg(test)]` boundary,
//! and `thing.rs` beside `thing/` rather than a `mod.rs` — rule 8.

#![allow(clippy::expect_used)]

use super::{AssignmentEpoch, GenerationId, GroupEvent, GroupState, MemberEpoch};
use crate::Error;

/// Every legal transition doc 02 §3.1 and this module's own doc name,
/// exhaustively — the acceptance criterion this task's own backlog row
/// asks for.
const LEGAL: &[(GroupState, GroupEvent, GroupState)] = &[
    (
        GroupState::Empty,
        GroupEvent::Join,
        GroupState::PreparingRebalance,
    ),
    (GroupState::Empty, GroupEvent::Expire, GroupState::Dead),
    (
        GroupState::PreparingRebalance,
        GroupEvent::JoinBarrierComplete,
        GroupState::CompletingRebalance,
    ),
    (
        GroupState::PreparingRebalance,
        GroupEvent::AllMembersGone,
        GroupState::Empty,
    ),
    (
        GroupState::PreparingRebalance,
        GroupEvent::MemberLeft,
        GroupState::PreparingRebalance,
    ),
    (
        GroupState::Stable,
        GroupEvent::MemberLeft,
        GroupState::PreparingRebalance,
    ),
    (
        GroupState::CompletingRebalance,
        GroupEvent::MemberLeft,
        GroupState::PreparingRebalance,
    ),
    (
        GroupState::CompletingRebalance,
        GroupEvent::SyncComplete,
        GroupState::Stable,
    ),
    (
        GroupState::CompletingRebalance,
        GroupEvent::MemberJoinedDuringSync,
        GroupState::PreparingRebalance,
    ),
    (
        GroupState::CompletingRebalance,
        GroupEvent::AllMembersGone,
        GroupState::Empty,
    ),
    (
        GroupState::Stable,
        GroupEvent::Join,
        GroupState::PreparingRebalance,
    ),
    (
        GroupState::Stable,
        GroupEvent::AllMembersGone,
        GroupState::Empty,
    ),
];

const ALL_STATES: &[GroupState] = &[
    GroupState::Empty,
    GroupState::PreparingRebalance,
    GroupState::CompletingRebalance,
    GroupState::Stable,
    GroupState::Dead,
];
const ALL_EVENTS: &[GroupEvent] = &[
    GroupEvent::Join,
    GroupEvent::JoinBarrierComplete,
    GroupEvent::SyncComplete,
    GroupEvent::MemberJoinedDuringSync,
    GroupEvent::MemberLeft,
    GroupEvent::AllMembersGone,
    GroupEvent::Expire,
];

/// `ALL_STATES`/`ALL_EVENTS` are hand-maintained arrays; this pins them
/// against the enums' own exhaustive-match guards so a variant added to
/// either enum without a matching array entry fails loudly here rather
/// than the exhaustiveness tests below silently covering less than they
/// claim to.
#[test]
fn the_hand_maintained_arrays_are_exhaustive() {
    for &s in ALL_STATES {
        s.assert_exhaustive();
    }
    for &e in ALL_EVENTS {
        e.assert_exhaustive();
    }
    assert_eq!(
        ALL_STATES.len(),
        5,
        "a GroupState variant was added or removed"
    );
    assert_eq!(
        ALL_EVENTS.len(),
        7,
        "a GroupEvent variant was added or removed"
    );
}

#[test]
fn every_legal_transition_succeeds() {
    for &(from, event, to) in LEGAL {
        let (got, ..) = from
            .transition(event, GenerationId::INITIAL, AssignmentEpoch::INITIAL)
            .expect("legal");
        assert_eq!(got, to, "{from:?} + {event:?}");
    }
}

#[test]
fn every_other_pair_is_refused() {
    for &state in ALL_STATES {
        for &event in ALL_EVENTS {
            let is_legal = LEGAL.iter().any(|&(s, e, _)| s == state && e == event);
            let result = state.transition(event, GenerationId::INITIAL, AssignmentEpoch::INITIAL);
            if is_legal {
                assert!(result.is_ok(), "{state:?} + {event:?} should be legal");
            } else {
                assert_eq!(
                    result,
                    Err(Error::IllegalGroupTransition {
                        state: format!("{state:?}"),
                        event: format!("{event:?}"),
                    }),
                    "{state:?} + {event:?} should be refused"
                );
            }
        }
    }
}

#[test]
fn the_generation_advances_only_on_join_barrier_complete() {
    let (_, same, _) = GroupState::Empty
        .transition(
            GroupEvent::Join,
            GenerationId::INITIAL,
            AssignmentEpoch::INITIAL,
        )
        .expect("legal");
    assert_eq!(same, GenerationId::INITIAL);

    let (_, bumped, _) = GroupState::PreparingRebalance
        .transition(
            GroupEvent::JoinBarrierComplete,
            GenerationId::INITIAL,
            AssignmentEpoch::INITIAL,
        )
        .expect("legal");
    assert_eq!(bumped.get(), GenerationId::INITIAL.get() + 1);
}

#[test]
fn the_assignment_epoch_advances_only_on_sync_complete_and_catches_up_to_the_generation() {
    let generation = GenerationId::INITIAL;
    let (_, _, unchanged) = GroupState::PreparingRebalance
        .transition(
            GroupEvent::JoinBarrierComplete,
            generation,
            AssignmentEpoch::INITIAL,
        )
        .expect("legal");
    // JoinBarrierComplete bumps the generation but not the assignment
    // epoch yet -- the lag this module's own doc names.
    assert_eq!(unchanged, AssignmentEpoch::INITIAL);

    // A generation past 1 -- `get`'s mutant that always returns a
    // constant survives against a bumped generation of exactly 1
    // (INITIAL.next() once), since that constant coincides with the
    // real answer; several `.next()` calls rules that out.
    let advanced_generation = generation.next().next().next();
    let (_, _, caught_up) = GroupState::CompletingRebalance
        .transition(
            GroupEvent::SyncComplete,
            advanced_generation,
            AssignmentEpoch::INITIAL,
        )
        .expect("legal");
    assert_eq!(caught_up.get(), advanced_generation.get());
    assert_eq!(caught_up.get(), 3);
}

#[test]
fn assignment_epoch_initial_is_behind_generation_initial() {
    assert_eq!(AssignmentEpoch::INITIAL.get(), -1);
    assert!(AssignmentEpoch::INITIAL.get() < GenerationId::INITIAL.get());
}

#[test]
fn dead_is_terminal() {
    for &event in ALL_EVENTS {
        assert!(
            GroupState::Dead
                .transition(event, GenerationId::INITIAL, AssignmentEpoch::INITIAL)
                .is_err()
        );
    }
}

#[test]
fn member_epoch_starts_at_a_fresh_join() {
    assert_eq!(MemberEpoch::INITIAL.get(), 0);
}
