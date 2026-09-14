//! The consumer-group coordination seam (`M4.2`, FR-20, FR-21;
//! `ADR-0034`).

use crate::{AssignmentEpoch, GenerationId, GroupEvent, GroupId, GroupState, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// One group's own state, generation, and assignment epoch.
///
/// The three values [`crate::GroupState::transition`] threads together,
/// bundled so a [`GroupCoordinator`] implementor holds and returns them as
/// one unit rather than three separately-fetched fields that could
/// disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupRecord {
    /// The group's own lifecycle state.
    pub state: GroupState,
    /// The classic protocol's own generation counter.
    pub generation: GenerationId,
    /// The generation that produced the current target assignment.
    pub assignment_epoch: AssignmentEpoch,
}

impl GroupRecord {
    /// A brand-new group's own starting record — `Empty`, at the initial
    /// generation and assignment epoch, before any event has ever reached
    /// it.
    const FRESH: Self = Self {
        state: GroupState::Empty,
        generation: GenerationId::INITIAL,
        assignment_epoch: AssignmentEpoch::INITIAL,
    };
}

/// Holds every group this node coordinates, and drives each one's own
/// state machine.
///
/// `ADR-0033`'s "every group resolves to this node" decision made real: one
/// coordinator instance, keyed by [`GroupId`], serving every group
/// `FindCoordinator` ever answers with this node.
///
/// ⚠️ **`transition` is the only route in.** There is no setter and no way
/// to construct a [`GroupRecord`] directly through this seam — the same "no
/// setter" discipline [`crate::MaterializedIndex`]'s own doc already states
/// for the metadata index, applied here so a caller cannot reach a state
/// [`GroupState::transition`]'s own legal-transition table would have
/// refused.
///
/// ⚠️ **Sans-I/O, mirroring [`crate::MaterializedIndex`], not
/// [`crate::MetadataLog`]** — `ADR-0034`. No implementation of this trait
/// does I/O in v1: durable storage across a restart is `M4.14`'s own
/// separate, not-yet-built task, and today's only implementation
/// ([`FakeGroupCoordinator`]) is in-memory, same as this trait's own
/// eventual real implementation until that task lands.
pub trait GroupCoordinator: Send + Sync + core::fmt::Debug {
    /// Drives `group`'s own state machine through `event`. A `group` with no
    /// prior record starts from [`GroupRecord::FRESH`] before the transition
    /// is attempted — there is no separate "create a group" call;
    /// [`GroupEvent::Join`] on an unknown id is how one comes to exist.
    ///
    /// # Errors
    ///
    /// [`crate::Error::IllegalGroupTransition`] if `event` has no legal
    /// successor from `group`'s current state — the record is left
    /// unchanged, the same "a refused fold leaves nothing applied"
    /// discipline [`crate::MaterializedIndex::apply`]'s own contract states.
    fn transition(&self, group: &GroupId, event: GroupEvent) -> Result<GroupRecord>;

    /// `group`'s current record, or `None` if it has never transitioned —
    /// never joined, or reaped past [`GroupState::Dead`] by a caller with
    /// the ability to forget it, which nothing in this milestone's own
    /// scope yet builds.
    fn record(&self, group: &GroupId) -> Option<GroupRecord>;
}

/// An in-memory [`GroupCoordinator`], faithful to the documented contract.
///
/// ⚠️ Here rather than in `oqueue-testkit` because `contracts.md` rule 9
/// puts a fake beside its trait: a downstream crate testing against this
/// must not have to depend on a future real implementation to get one.
#[derive(Default)]
pub struct FakeGroupCoordinator {
    groups: Mutex<HashMap<GroupId, GroupRecord>>,
    /// How many times [`GroupCoordinator::transition`] has been called,
    /// successful or refused — `M4.10`'s own need: "one rebalance, not
    /// three" for a batched `LeaveGroup` is not observable from the
    /// resulting state alone (a buggy implementation calling `transition`
    /// once per removed member would have its second and third calls
    /// silently refused, landing on the identical final state), so a
    /// caller asserting "called exactly once" needs this counter, the
    /// same "a proxy, not wall-clock timing" idiom `Cluster::topic_lookups`
    /// (`M9.17`) already established for an analogous question.
    transition_calls: AtomicU64,
}

impl FakeGroupCoordinator {
    /// A coordinator holding no groups.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times [`GroupCoordinator::transition`] has been called on
    /// this coordinator so far, whatever the outcome.
    #[must_use]
    pub fn transition_calls(&self) -> u64 {
        self.transition_calls.load(Ordering::Relaxed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<GroupId, GroupRecord>> {
        self.groups.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl core::fmt::Debug for FakeGroupCoordinator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FakeGroupCoordinator")
            .field("groups", &self.lock().len())
            .finish()
    }
}

impl GroupCoordinator for FakeGroupCoordinator {
    fn transition(&self, group: &GroupId, event: GroupEvent) -> Result<GroupRecord> {
        self.transition_calls.fetch_add(1, Ordering::Relaxed);
        let mut groups = self.lock();
        let current = groups.get(group).copied().unwrap_or(GroupRecord::FRESH);
        let (state, generation, assignment_epoch) =
            current
                .state
                .transition(event, current.generation, current.assignment_epoch)?;
        let record = GroupRecord {
            state,
            generation,
            assignment_epoch,
        };
        groups.insert(group.clone(), record);
        drop(groups);
        Ok(record)
    }

    fn record(&self, group: &GroupId) -> Option<GroupRecord> {
        self.lock().get(group).copied()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{FakeGroupCoordinator, GroupRecord};
    use crate::{Error, GenerationId, GroupCoordinator, GroupEvent, GroupId, GroupState};

    fn group(name: &str) -> GroupId {
        GroupId::new(name).expect("valid")
    }

    #[test]
    fn debug_reports_the_group_count() {
        let c = FakeGroupCoordinator::new();
        assert!(format!("{c:?}").contains('0'));
        c.transition(&group("orders"), GroupEvent::Join)
            .expect("legal");
        assert!(format!("{c:?}").contains('1'));
    }

    #[test]
    fn an_unknown_group_has_no_record() {
        let c = FakeGroupCoordinator::new();
        assert_eq!(c.record(&group("orders")), None);
    }

    #[test]
    fn a_join_on_an_unknown_group_creates_it_at_empty_first() {
        let c = FakeGroupCoordinator::new();
        let record = c
            .transition(&group("orders"), GroupEvent::Join)
            .expect("Empty -> Join is legal");
        assert_eq!(record.state, GroupState::PreparingRebalance);
        assert_eq!(record.generation, GenerationId::INITIAL);
    }

    #[test]
    fn a_refused_transition_leaves_the_record_unchanged() {
        let c = FakeGroupCoordinator::new();
        c.transition(&group("orders"), GroupEvent::Join)
            .expect("Empty -> Join is legal");
        let before = c.record(&group("orders")).expect("exists");

        let refused = c.transition(&group("orders"), GroupEvent::SyncComplete);
        assert_eq!(
            refused,
            Err(Error::IllegalGroupTransition {
                state: format!("{:?}", GroupState::PreparingRebalance),
                event: format!("{:?}", GroupEvent::SyncComplete),
            })
        );
        assert_eq!(c.record(&group("orders")), Some(before));
    }

    #[test]
    fn two_groups_do_not_share_state() {
        let c = FakeGroupCoordinator::new();
        c.transition(&group("orders"), GroupEvent::Join)
            .expect("legal");
        assert_eq!(c.record(&group("payments")), None);
    }

    #[test]
    fn transition_calls_counts_every_call_including_refused_ones() {
        let c = FakeGroupCoordinator::new();
        assert_eq!(c.transition_calls(), 0);
        c.transition(&group("orders"), GroupEvent::Join)
            .expect("Empty -> Join is legal");
        assert_eq!(c.transition_calls(), 1);
        // Refused: Join has no legal successor from PreparingRebalance.
        let _ = c.transition(&group("orders"), GroupEvent::Join);
        assert_eq!(
            c.transition_calls(),
            2,
            "a refused call still counts -- it was still called"
        );
    }

    fn arbitrary_event() -> impl proptest::strategy::Strategy<Value = GroupEvent> {
        proptest::prelude::prop_oneof![
            proptest::prelude::Just(GroupEvent::Join),
            proptest::prelude::Just(GroupEvent::JoinBarrierComplete),
            proptest::prelude::Just(GroupEvent::SyncComplete),
            proptest::prelude::Just(GroupEvent::MemberJoinedDuringSync),
            proptest::prelude::Just(GroupEvent::MemberLeft),
            proptest::prelude::Just(GroupEvent::AllMembersGone),
            proptest::prelude::Just(GroupEvent::Expire),
        ]
    }

    proptest::proptest! {
        /// The acceptance criterion this task's own backlog row asks for,
        /// as a property rather than a fixed example: for *any* sequence of
        /// events, the coordinator's own result at every step is exactly
        /// what sequentially applying `GroupState::transition` directly
        /// would produce -- an accepted event lands on the same record, a
        /// refused one leaves the record exactly where it was and returns
        /// the identical error. No method on the trait can therefore ever
        /// reach a state the state machine's own legal-transition table
        /// would have refused, because the coordinator is never anything
        /// but that table applied one event at a time.
        #[test]
        fn the_coordinator_never_diverges_from_the_state_machine_it_wraps(
            events in proptest::collection::vec(arbitrary_event(), 0..20)
        ) {
            let c = FakeGroupCoordinator::new();
            let g = group("orders");
            let mut expected = GroupRecord::FRESH;

            for event in events {
                let direct = expected
                    .state
                    .transition(event, expected.generation, expected.assignment_epoch);
                let via_coordinator = c.transition(&g, event);
                match direct {
                    Ok((state, generation, assignment_epoch)) => {
                        let record = GroupRecord { state, generation, assignment_epoch };
                        proptest::prop_assert_eq!(via_coordinator, Ok(record));
                        expected = record;
                    }
                    Err(error) => {
                        proptest::prop_assert_eq!(via_coordinator, Err(error));
                        // The record is unchanged on a refusal -- `expected`
                        // stays put too, matching the coordinator's own
                        // "a refused fold leaves nothing applied" contract.
                    }
                }
            }
        }
    }

    /// The same property, spot-checked exhaustively from a fresh group for
    /// every event, without a sequence to draw -- a readable fixed example
    /// beside the property test above.
    #[test]
    fn only_the_state_machine_s_own_legal_transitions_are_ever_reachable() {
        const EVENTS: &[GroupEvent] = &[
            GroupEvent::Join,
            GroupEvent::JoinBarrierComplete,
            GroupEvent::SyncComplete,
            GroupEvent::MemberJoinedDuringSync,
            // ⚠️ **`MemberLeft` is illegal from `Empty`, so this loop only
            // ever checks that both sides refuse it.** The block after the
            // loop reaches its legal arm.
            GroupEvent::MemberLeft,
            GroupEvent::AllMembersGone,
            GroupEvent::Expire,
        ];
        // A fresh coordinator per event, so each check is independent of
        // any earlier event's own success -- both sides always start from
        // Empty at the initial generation and assignment epoch.
        for &event in EVENTS {
            let direct = GroupState::Empty.transition(
                event,
                GenerationId::INITIAL,
                crate::AssignmentEpoch::INITIAL,
            );
            let via_coordinator = FakeGroupCoordinator::new().transition(&group("orders"), event);
            match (direct, via_coordinator) {
                (Ok((state, generation, assignment_epoch)), Ok(record)) => {
                    assert_eq!(
                        record,
                        GroupRecord {
                            state,
                            generation,
                            assignment_epoch,
                        }
                    );
                }
                (Err(_), Err(_)) => {}
                (direct, via_coordinator) => panic!(
                    "the coordinator and the state machine disagree for {event:?}: {direct:?} vs {via_coordinator:?}"
                ),
            }
        }
    }

    /// ⚠️ **Its own test because the loop above cannot reach this arm**, and
    /// because folding it in took that function past
    /// `clippy::too_many_lines`.
    #[test]
    fn the_fake_agrees_with_the_table_on_the_one_state_preserving_arm() {
        // ⚠️ **The state-preserving arm, which the loop above cannot reach**
        // — every event there is applied to a *fresh* coordinator, so both
        // sides start from `Empty` and `(Empty, MemberLeft)` lands in the
        // `(Err(_), Err(_))` case having compared nothing. This drives the
        // group to `PreparingRebalance` first and compares the one arm
        // nothing else exercises.
        //
        // ⚠️ **It does not catch the divergence `M4.34`'s own comment named,
        // and nothing can.** That comment said a fake skipping its `insert`
        // when the successor equals the current state "would answer `None`
        // where the real coordinator answers `Some(PreparingRebalance)`".
        // Measured by planting exactly that skip in `FakeGroupCoordinator`:
        // the suite still passes. The arm preserves the generation and the
        // assignment epoch as well as the state, and the group already has
        // an entry by the time it is reached, so the write that is skipped
        // is byte-identical to the record already there — there is no
        // observer. The claim was not merely untested; it describes an
        // unobservable difference. ⚠️ What this block *does* buy is
        // agreement on an arm the loop cannot reach at all. `M4.34` wrote
        // the claim, `M4.55` found nothing tested it, and `M4.62` reached
        // the arm and measured why the original argument cannot be tested.
        let coordinator = FakeGroupCoordinator::new();
        let g = group("orders");
        coordinator
            .transition(&g, GroupEvent::Join)
            .expect("Empty -> Join is legal");
        let before = coordinator.record(&g).expect("the group exists now");
        assert_eq!(
            before.state,
            GroupState::PreparingRebalance,
            "the fixture must reach the state whose MemberLeft arm preserves it"
        );
        let direct = before.state.transition(
            GroupEvent::MemberLeft,
            before.generation,
            before.assignment_epoch,
        );
        let via_coordinator = coordinator.transition(&g, GroupEvent::MemberLeft);
        match (direct, via_coordinator) {
            (Ok((state, generation, assignment_epoch)), Ok(record)) => assert_eq!(
                record,
                GroupRecord {
                    state,
                    generation,
                    assignment_epoch,
                },
                "the fake and the table must agree on the one arm the loop above cannot reach"
            ),
            (direct, via_coordinator) => panic!(
                "the coordinator and the state machine disagree for MemberLeft from \
             PreparingRebalance: {direct:?} vs {via_coordinator:?}"
            ),
        }
    }
}
