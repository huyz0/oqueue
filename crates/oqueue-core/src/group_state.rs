//! The consumer group state machine (`M4.1`, FR-22; `ADR-0033`).
//!
//! Doc 02 §3.1: "the group's internal state machine moves through named
//! states: Empty (no members) → `PreparingRebalance` (waiting for
//! `JoinGroup`s) → `CompletingRebalance` (waiting for the leader's
//! `SyncGroup`) → Stable (steady state) → Dead (terminal, on group
//! deletion/cleanup)."
//!
//! ⚠️ **`ADR-0033`'s three-epoch model, not a single counter — built now,
//! even though only the classic protocol drives it in v1.** `GenerationId`
//! is Group Epoch's own classic-protocol wire name (doc 02 §3.1: it
//! "increments on every completed rebalance and is echoed by clients as a
//! fencing token" — the exact role Group Epoch plays). [`AssignmentEpoch`]
//! is the Group Epoch value that produced the *current target assignment*;
//! it lags `GenerationId` between `JoinBarrierComplete` (a new target is
//! now owed) and `SyncComplete` (the target for this generation actually
//! exists). [`MemberEpoch`] is each member's own progress toward that
//! target — a per-member value, not part of this module's own group-level
//! state, `M4.2`'s `GroupCoordinator` is where one is held per member and
//! `M4.9`'s `Heartbeat` handler is where it actually advances; the type
//! exists here so nothing downstream invents its own shape for it.
//!
//! ⚠️ **Only the legal transitions, nothing else** — a real handler
//! (`M4.7`-`M4.10`) drives this type through [`GroupState::transition`]
//! rather than constructing a successor state directly, so an event this
//! table does not name is refused rather than silently accepted. `M4`'s own
//! backlog row for this task asks for exactly that: "every `(state, event)`
//! pair either names its successor or is refused."
//!
//! ⚠️ **No `DeleteGroups`-style admin API exists in this milestone's own
//! scope** (`M4.md`'s eighteen tasks name none), so [`GroupEvent::Expire`]
//! — the only path to [`GroupState::Dead`] — has no live caller yet. The
//! seam is real, not invented: a future admin surface (`M12`'s own scope)
//! calls it, the same "seam now, caller later" shape `oqueue_core::authorize`
//! and `TopicGrants::grant`/`revoke` already established for this crate.

use crate::{Error, Result};

/// The classic protocol's own generation counter — Group Epoch's wire name.
///
/// ⚠️ **`ADR-0033`: derived from Group Epoch, not tracked as a second,
/// independent counter.** [`GroupState::transition`] is the only place this
/// advances, and it does so exactly when a rebalance's join barrier closes
/// — the same moment doc 02 §3.1 says real Kafka's own `GenerationId`
/// increments (`initNextGeneration`, not the later `SyncGroup` step).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GenerationId(i32);

impl GenerationId {
    /// The generation a brand-new, empty group starts at.
    pub const INITIAL: Self = Self(0);

    /// The wire value real Kafka clients read and echo.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }

    /// One generation later — `PreparingRebalance` → `CompletingRebalance`'s
    /// own moment, the only place this is ever called.
    #[must_use]
    const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

/// The [`GenerationId`] value that produced the group's *current target
/// assignment* (`ADR-0033`'s Assignment Epoch).
///
/// ⚠️ **Lags `GenerationId` between a join barrier closing and a sync
/// completing.** A new target is owed the moment `GenerationId` advances
/// (`JoinBarrierComplete`) but does not exist until the leader's
/// `SyncGroup` is relayed (`SyncComplete`) — [`GroupState::transition`]
/// only advances this at the second moment, never the first, so a caller
/// can always tell "a rebalance started" from "a rebalance produced an
/// assignment" by comparing this against the current `GenerationId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssignmentEpoch(i32);

impl AssignmentEpoch {
    /// No assignment has ever been computed — a brand-new, empty group's
    /// own starting value, behind [`GenerationId::INITIAL`] by construction
    /// (there is nothing to be caught up to yet).
    pub const INITIAL: Self = Self(-1);

    /// The wire-facing value.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

/// One member's own progress toward the group's current target assignment
/// (`ADR-0033`'s Member Epoch).
///
/// A fencing token on that member's own requests, the same role
/// `GenerationId` plays for the group as a whole.
///
/// ⚠️ **Not part of [`GroupState`]'s own value.** The group has one state
/// and one generation; it has as many member epochs as it has members, so
/// this type exists for `M4.2`'s `GroupCoordinator` to hold one per member,
/// not for this module to track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberEpoch(i32);

impl MemberEpoch {
    /// A freshly-joined member that has not yet synced to any target.
    pub const INITIAL: Self = Self(0);

    /// The wire-facing value.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

/// One event a group's state may be driven by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupEvent {
    /// A member joined — the first one (from `Empty`), or a membership
    /// change on an already-`Stable` group.
    Join,
    /// The join barrier is satisfied: every expected member has joined, or
    /// `rebalance.timeout.ms` elapsed with at least one (`M4.7`).
    JoinBarrierComplete,
    /// The leader's `SyncGroup` has been relayed to every member (`M4.8`).
    SyncComplete,
    /// A member joined while the group was `CompletingRebalance`, still
    /// waiting on the leader's `SyncGroup` — real Kafka re-opens the join
    /// barrier rather than completing sync against a membership that has
    /// already changed.
    MemberJoinedDuringSync,
    /// Every member has left or been evicted (`M4.9`, `M4.10`), whichever
    /// state that leaves the group in.
    AllMembersGone,
    /// An empty group past its own retention window is reaped. No caller in
    /// this milestone's own scope; see this module's own doc.
    Expire,
}

impl GroupEvent {
    /// Matches every variant with no wildcard — the "let a new variant fail
    /// to compile" idiom `AGENTS.md`'s own `Error` enum uses non-exhaustively
    /// applied to this one. Exists purely so `tests::all_events` cannot go
    /// silently stale: a `GroupEvent` added without a matching arm here
    /// fails the build, which is the prompt to also extend the transition
    /// table and the test's own hand-maintained array together.
    #[cfg(test)]
    const fn assert_exhaustive(self) {
        match self {
            Self::Join
            | Self::JoinBarrierComplete
            | Self::SyncComplete
            | Self::MemberJoinedDuringSync
            | Self::AllMembersGone
            | Self::Expire => {}
        }
    }
}

/// The group's own lifecycle state, doc 02 §3.1's five named states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupState {
    /// No members.
    Empty,
    /// Waiting for `JoinGroup`s.
    PreparingRebalance,
    /// Waiting for the leader's `SyncGroup`.
    CompletingRebalance,
    /// Steady state: every member has its assignment.
    Stable,
    /// Terminal.
    Dead,
}

impl GroupState {
    /// The same exhaustiveness guard as [`GroupEvent::assert_exhaustive`],
    /// for this enum.
    #[cfg(test)]
    const fn assert_exhaustive(self) {
        match self {
            Self::Empty
            | Self::PreparingRebalance
            | Self::CompletingRebalance
            | Self::Stable
            | Self::Dead => {}
        }
    }

    /// Attempts `event` against this state, returning the successor state,
    /// the generation to carry forward, and the assignment epoch to carry
    /// forward. The generation advances only at `JoinBarrierComplete`; the
    /// assignment epoch advances only at `SyncComplete`, catching up to
    /// whatever generation is current at that moment — the two-step lag
    /// this module's own doc names.
    ///
    /// # Errors
    ///
    /// [`Error::IllegalGroupTransition`] if `event` has no legal successor
    /// from this state.
    pub fn transition(
        self,
        event: GroupEvent,
        generation: GenerationId,
        assignment_epoch: AssignmentEpoch,
    ) -> Result<(Self, GenerationId, AssignmentEpoch)> {
        use GroupEvent::{
            AllMembersGone, Expire, Join, JoinBarrierComplete, MemberJoinedDuringSync, SyncComplete,
        };
        use GroupState::{CompletingRebalance, Dead, Empty, PreparingRebalance, Stable};

        match (self, event) {
            // A fresh join (Empty) and a membership change re-opening an
            // already-negotiated group (CompletingRebalance mid-sync,
            // Stable) all land on the same barrier — doc 02 §3.1's own
            // "waiting for JoinGroups" state, regardless of which of the
            // three moments actually opened it.
            (Empty | Stable, Join) | (CompletingRebalance, MemberJoinedDuringSync) => {
                Ok((PreparingRebalance, generation, assignment_epoch))
            }
            (Empty, Expire) => Ok((Dead, generation, assignment_epoch)),
            (PreparingRebalance, JoinBarrierComplete) => {
                Ok((CompletingRebalance, generation.next(), assignment_epoch))
            }
            // Membership loss resets to Empty from every state that has
            // members but no completed sync to protect — PreparingRebalance
            // and Stable, the two round 1 review already covered, and
            // CompletingRebalance beside them: a group's only member
            // leaving or timing out mid-sync (after JoinBarrierComplete,
            // before SyncComplete) is the same real event as the other two,
            // and round 1 review found it missing here.
            (PreparingRebalance | Stable | CompletingRebalance, AllMembersGone) => {
                Ok((Empty, generation, assignment_epoch))
            }
            (CompletingRebalance, SyncComplete) => Ok((Stable, generation, generation.into())),
            (state, event) => Err(Error::IllegalGroupTransition {
                state: format!("{state:?}"),
                event: format!("{event:?}"),
            }),
        }
    }
}

impl From<GenerationId> for AssignmentEpoch {
    /// The assignment epoch catching up to `generation` at `SyncComplete` —
    /// [`GroupState::transition`]'s own use, not a general conversion
    /// callers should reach for elsewhere.
    fn from(generation: GenerationId) -> Self {
        Self(generation.get())
    }
}

#[cfg(test)]
mod tests {
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
            6,
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
                let result =
                    state.transition(event, GenerationId::INITIAL, AssignmentEpoch::INITIAL);
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
}
