//! The consumer group state machine (`M4.1`, FR-20; `ADR-0033`).
//!
//! ⚠️ **FR-22 (KIP-848) is `deferred` and this module does not implement it —
//! but it is the reason for this shape.** `ADR-0033` decision 2 asks for the
//! three-epoch model (group, assignment, member) from the first commit even
//! though only the classic protocol drives it in v1, so that a later KIP-848
//! pickup is a coexistence addition rather than a rewrite: the coordinator
//! translates classic Join/Sync/Heartbeat onto the new group model, and doing
//! it the other way round is what a single `GenerationId` would have forced.
//! So the citation here is *shaped by a deferred requirement*, which is not
//! the same claim as serving it — `M4.25`, after seven sibling modules were
//! found citing FR-22 for work that is plainly FR-20's.
//!
//! Doc 02 §3.1: "the group's internal state machine moves through named
//! states: Empty (no members) → `PreparingRebalance` (waiting for
//! `JoinGroup`s) → `CompletingRebalance` (waiting for the leader's
//! `SyncGroup`) → Stable (steady state) → Dead (terminal, on group
//! deletion/cleanup)."
//!
//! ⚠️ **`ADR-0033` asks for a three-epoch model, not a single counter, and
//! ⚠️ two of the three are what this module actually threads.** `GenerationId`
//! is Group Epoch's own classic-protocol wire name (doc 02 §3.1: it
//! "increments on every completed rebalance and is echoed by clients as a
//! fencing token" — the exact role Group Epoch plays). [`AssignmentEpoch`]
//! is the Group Epoch value that produced the *current target assignment*;
//! it lags `GenerationId` between `JoinBarrierComplete` (a new target is
//! now owed) and `SyncComplete` (the target for this generation actually
//! exists). [`MemberEpoch`] is each member's own progress toward that target — a
//! per-member value, not part of this module's own group-level state.
//! ⚠️ **Nothing holds one and nothing advances one.** This paragraph said
//! `M4.2`'s `GroupCoordinator` held one per member and `M4.9`'s `Heartbeat`
//! advanced it; both shipped and neither does, which M4's closing review
//! found and `M4.39` corrects here. `GroupRecord` is state, generation and
//! assignment epoch, and a whole-workspace grep finds this type only in its
//! own module and one re-export. Whether to build the member epoch or to
//! amend `ADR-0033` to say the classic protocol needs two is a decision,
//! argued as `b0b5af3ced25` in `baselines/review.txt` and received by
//! `roadmap.md`'s "Deferred, with nothing scheduled".
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
/// ⚠️ **Not part of [`GroupState`]'s own value**, and ⚠️ **not held
/// anywhere else either.** The group has one state and one generation; it
/// would have as many member epochs as it has members, and this type was
/// added for `M4.2`'s `GroupCoordinator` to hold one per member. `M4.2`
/// shipped and holds none — see this module's own doc, and the argued
/// finding it names.
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
    /// A member left or was evicted while others remain — `M4.10`'s
    /// `LeaveGroup` and `M4.9`'s session-timeout sweep, which reach it
    /// through one place (`Heartbeats::remove_where`).
    ///
    /// ⚠️ **Distinct from [`GroupEvent::Join`], which is what the broker
    /// used to fire for this** — the state machine had no event for
    /// leaving, so eviction borrowed the one for joining. That is correct
    /// only from `Stable`: a partial membership loss from
    /// `PreparingRebalance` or `CompletingRebalance` took a transition the
    /// table refuses, so the coordinator was never told and a group whose
    /// leader died mid-sync held its generation with no leader. Real
    /// Kafka's `onExpireHeartbeat` handles those states explicitly.
    /// `M4.34`, found by M4's closing review.
    MemberLeft,
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
            | Self::MemberLeft
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
            AllMembersGone, Expire, Join, JoinBarrierComplete, MemberJoinedDuringSync, MemberLeft,
            SyncComplete,
        };
        use GroupState::{CompletingRebalance, Dead, Empty, PreparingRebalance, Stable};

        match (self, event) {
            // A fresh join (Empty) and a membership change re-opening an
            // already-negotiated group (CompletingRebalance mid-sync,
            // Stable) all land on the same barrier — doc 02 §3.1's own
            // "waiting for JoinGroups" state, regardless of which of the
            // three moments actually opened it.
            // ⚠️ **`MemberLeft` is admitted from every state that still
            // has members, including the two a rebalance is already under
            // way in** — one arm with the joins above because the answer is
            // the same barrier, which is also what `clippy::match_same_arms`
            // insists on. From `Stable` it does what `Join` did for a
            // partial loss and the outcome is unchanged; the reason the
            // event exists is the other two. A member lost from
            // `CompletingRebalance` must reopen the barrier for exactly the
            // reason `MemberJoinedDuringSync` does — the leader is
            // computing an assignment against a membership that no longer
            // holds. From `PreparingRebalance` the round's own `awaiting`
            // set has already stopped waiting for it (`M4.16`), so the
            // self-transition changes no state; what it buys is the
            // coordinator agreeing rather than refusing an event nobody
            // reads the result of. `M4.34`.
            (Empty | Stable, Join)
            | (CompletingRebalance, MemberJoinedDuringSync)
            | (PreparingRebalance | Stable | CompletingRebalance, MemberLeft) => {
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
mod tests;
