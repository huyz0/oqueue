//! The join barrier's own data: what a round holds, what a join answers with,
//! and the two bounds the retry loops obey.
//!
//! ⚠️ **Split from `super` by `M4.15d` for `code-structure.md` rule 16.**
//! Keeping the types beside the async orchestration put that file at 524
//! lines; the seam is that nothing here awaits anything. `super` owns the
//! `.await`s, `plan` owns the decisions, and this owns what both operate on.

use oqueue_core::{GenerationId, GroupCoordinator, GroupEvent};

/// What a round publishes when it ends: the close it elected, or `None` if it
/// was abandoned.
///
/// ⚠️ **Abandonment has to be publishable, not merely a dropped round.**
/// `abandon_round` used to take `entry.open` and notify, leaving `outcome`
/// unset — so every *other* member enrolled in that round stayed parked until
/// its own full rebalance timeout (up to `MAX_BARRIER_MS`) and was then
/// answered `UNKNOWN_SERVER_ERROR`, which the Java consumer does not treat as
/// retriable. The task that fired the barrier recovered; its co-members did
/// not. Found by review.
pub(crate) type RoundOutcome = Option<Arc<RoundClose>>;
use std::sync::{Arc, OnceLock};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// One member's own contribution to a round: what a response echoes back
/// for it, and what `elect` needs from it.
#[derive(Debug, Clone)]
pub(crate) struct RoundMember {
    pub(crate) member_id: String,
    pub(crate) protocol_type: String,
    pub(crate) protocols: Vec<(String, Vec<u8>)>,
}

/// What a closed round answers every one of its own members with.
#[derive(Debug, Clone)]
pub(crate) struct RoundClose {
    pub(crate) generation: GenerationId,
    pub(crate) leader: String,
    pub(crate) protocol_type: String,
    pub(crate) protocol_name: String,
    pub(crate) members: Vec<RoundMember>,
}

/// A group's own currently-collecting round.
#[derive(Debug)]
pub(super) struct OpenRound {
    pub(super) members: Vec<RoundMember>,
    /// How many members close this round the moment they join, without
    /// waiting for `deadline` — `None` when nothing is known yet, which
    /// leaves `deadline` as the only way this round ever closes.
    pub(super) expected: Option<usize>,
    pub(super) deadline: Instant,
    pub(super) notify: Arc<Notify>,
    pub(super) outcome: Arc<OnceLock<RoundOutcome>>,
}

/// One group's own join-round bookkeeping.
#[derive(Debug, Default)]
pub(super) struct Entry {
    pub(super) open: Option<OpenRound>,
    /// How many members closed this group's most recently *elected* round —
    /// the next round's own `expected`, until it closes and updates this.
    ///
    /// ⚠️ **`None` for a group that has never closed one — not `Some(1)`.**
    /// A group's true first-ever round has no prior membership to know a
    /// count from, so it waits out its own deadline like every other
    /// unresolved round rather than closing the instant its own opener
    /// joins; real Kafka's `group.initial.rebalance.delay.ms` names the
    /// same gap for the same reason (batching concurrently-starting
    /// consumers into one round), and this is that gap, stood in for by
    /// `rebalance_timeout_ms` itself rather than a second, not-yet-built
    /// configuration value.
    pub(super) last_round_size: Option<usize>,
    /// Set while a transition for this group is in flight through
    /// [`crate::group_transitions::GroupTransitions`] — `M4.15d`.
    ///
    /// ⚠️ **This is what replaces "the decision and the transition happen
    /// under one lock".** Routing through the actor makes the transition
    /// `async`, and `async-concurrency.md` rule 6 forbids holding this
    /// module's `std::sync::Mutex` across that `.await` — so the decision
    /// and its application are no longer one atomic step, and two
    /// concurrent joiners could each decide "no round is open, fire
    /// `Join`" and durably apply it twice. Marking the group in-flight
    /// under the lock, before releasing it, is what makes the pair atomic
    /// again: a second joiner sees this, waits on it, and re-plans against
    /// the state the first one produced.
    ///
    /// ⚠️ **Not a lock wearing a different hat.** It is held across an
    /// `.await` by nothing — it is a flag other tasks *read* under the
    /// same mutex, and the waiting happens with that mutex released.
    pub(super) in_flight: Option<Arc<Notify>>,
}

/// What [`GroupJoins::join`] hands back to a caller.
pub(crate) enum JoinOutcome {
    /// The group's own state could not be advanced because a *dependency*
    /// failed, not because anything about this request was wrong — the log is
    /// unreachable, or the transition actor is gone.
    ///
    /// ⚠️ **Its own variant because the wire code differs and one of them is
    /// fatal.** `Refused` maps to `INCONSISTENT_GROUP_PROTOCOL`, which the
    /// Java consumer raises out of `poll()` and never retries; answering that
    /// to an object-storage outage kills every consumer in the group and they
    /// do not come back when storage does. This maps to
    /// `COORDINATOR_NOT_AVAILABLE`, which is retriable. Found by review:
    /// before `M4.15d` this path called the pure in-memory coordinator, whose
    /// only failure was `Dead`, so one mapping was enough.
    Unavailable,
    /// This join was overtaken too many times to keep re-planning, or spent
    /// its whole slot-wait budget without the group's own in-flight slot
    /// coming free.
    ///
    /// ⚠️ **Its own variant, and `REBALANCE_IN_PROGRESS` rather than
    /// `Refused`'s `INCONSISTENT_GROUP_PROTOCOL`.** Exhaustion means the group
    /// is *busy*, which is precisely what a client should rejoin on. Falling
    /// into `Refused` made a slow actor fatal: one lapsed budget consumes
    /// every remaining pass instantly, so a second joiner arriving during an
    /// object-store latency spike was killed rather than asked to retry.
    /// Found by review, twice — the second time as a consequence of bounding
    /// the wait, which is why the exhaustion arm now says what it means.
    Busy,
    /// This join itself closed the round — every waiter, including the
    /// caller, has an answer already.
    Ready(Arc<RoundClose>),
    /// The round is still collecting; wait on `notify` or `deadline`,
    /// whichever comes first, then read `outcome`.
    Pending {
        notify: Arc<Notify>,
        deadline: Instant,
        outcome: Arc<OnceLock<RoundOutcome>>,
    },
    /// This member was never enrolled — either it shares no protocol with
    /// the round's own running candidate set (`INCONSISTENT_GROUP_PROTOCOL`,
    /// the caller's job to map), or the coordinator's own transition table
    /// refused the event outright (`Dead`, unreachable in this milestone,
    /// this module's own bug rather than a case a client can trigger).
    Refused,
}

/// How many times [`GroupJoins::join`] re-plans after losing a race to
/// another joiner's own in-flight transition — `MAX_COMMIT_RETRIES`'s own
/// instinct (`async-concurrency.md` rule 13): a caller that never stops
/// retrying is an unbounded hold wearing a retry loop.
///
/// ⚠️ **A retry here is a *re-plan*, never a re-apply.** Each pass reads the
/// group's own state afresh and decides again; the pass that loses the race
/// applies nothing at all. So the budget bounds how many times this task is
/// overtaken, not how many durable records it can write.
pub(super) const MAX_JOIN_REPLANS: usize = 8;

/// The longest a deadline-driven close waits for whoever already holds this
/// group's own in-flight slot to finish.
///
/// ⚠️ **A liveness bound, not a tuning knob.** It exists so a slot that is
/// never released cannot hang a request forever. ⚠️ **The wait it bounds is
/// not microseconds**, which an earlier version of this comment claimed: the
/// holder is awaiting `GroupTransitions`, whose `handle_one` does a
/// `last_version` read plus an `append` against object storage, serialized
/// behind every other group's transitions in one `mpsc` actor. Tens of
/// milliseconds is the good case and seconds is a plausible bad one, so 5 s
/// is a margin rather than the enormous one the old wording implied.
/// Lowering it makes a waiter give up on a close that was genuinely coming
/// and answer `REBALANCE_IN_PROGRESS`; raising it lengthens how long one
/// request can be held by a bug elsewhere.
pub(super) const SLOT_WAIT: Duration = Duration::from_secs(5);

/// The two collaborators every round operation needs, bundled because they
/// always travel together and never apart: the actor that applies a
/// transition, and the coordinator whose state decides which one to fire.
///
/// ⚠️ Introduced because `join` reached six arguments against
/// `rust-style.md`'s own five — a limit doing its job here rather than
/// being worked around, since "which coordinator" and "which actor" are one
/// decision and passing them separately invites answering it twice.
#[derive(Clone, Copy)]
pub(crate) struct Coordination<'a> {
    pub(crate) transitions: &'a crate::group_transitions::GroupTransitions,
    pub(crate) coordinator: &'a dyn GroupCoordinator,
}

/// One pass of [`GroupJoins::join`]'s own plan, decided under the lock and
/// carried out with it released.
pub(super) enum Step {
    /// Another task holds this group's own in-flight transition. Wait on
    /// its `Notify`, then plan again against whatever it produced.
    Wait(Arc<Notify>),
    /// No round is open: fire `event` through the actor, then plan again.
    /// ⚠️ The round is deliberately *not* installed until the transition
    /// has durably landed — installing first and rolling back on failure
    /// would make a refused join briefly visible as an open round.
    Open(GroupEvent),
    /// The member is enrolled and this join filled the round: fire
    /// `JoinBarrierComplete`, then finalize under the lock.
    Close,
    /// Nothing further to do — hand this outcome to the caller.
    Done(JoinOutcome),
}
