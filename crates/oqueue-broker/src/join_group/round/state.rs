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
/// for it, what `elect` needs from it, and what it asked the sync barrier to
/// wait for.
#[derive(Debug, Clone)]
pub(crate) struct RoundMember {
    pub(crate) member_id: String,
    pub(crate) protocol_type: String,
    pub(crate) protocols: Vec<(String, Vec<u8>)>,
    /// This member's own `rebalance_timeout_ms`, already through
    /// `super::super::deadline::barrier_ms`.
    ///
    /// ⚠️ **Here since `M4.83`, and the reason is that the fold had nowhere
    /// to be recomputed from.** `SyncGroups` folded `max` over every member
    /// that ever enrolled and nothing ever lowered it, so a member that
    /// reached `Pending` asking the ceiling and then disconnected held its
    /// group's sync backstop there for the life of the process. The repair is
    /// to derive the group's number from the round's *current* roster, and
    /// that needs each member to carry its own — `M4.70`'s and `M4.76`'s rows
    /// each proposed snapshotting `members` and folding over it, which could
    /// not work while there was nothing on a member to fold.
    pub(crate) rebalance_timeout: Duration,
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
    /// The member ids this round is still waiting for — the previous
    /// round's own roster, minus whoever has rejoined so far.
    ///
    /// ⚠️ **A set, not a count, and `M4.16` is why.** This was
    /// `expected: Option<usize>` compared against `members.len()`, which
    /// closes a round on *whoever* arrives first: a newcomer joining a
    /// one-member group closed the round on itself alone, became leader of a
    /// roster of one, and took every partition — while the member that
    /// actually held them was never asked to revoke anything and went on
    /// consuming them until its next heartbeat was fenced. Then it rejoined,
    /// closed a round alone in turn, and took them back. Two consumers never
    /// converged, and both consumed the same partitions in between. FR-20's
    /// invariant — a partition is revoked by its previous owner before it is
    /// handed to a new one — failing with no mixed protocols in sight.
    ///
    /// `None` means nothing is known yet, which leaves the deadline as the
    /// only way the round closes.
    pub(super) awaiting: Option<Vec<String>>,
    pub(super) deadline: Instant,
    pub(super) notify: Arc<Notify>,
    pub(super) outcome: Arc<OnceLock<RoundOutcome>>,
}

/// One group's own join-round bookkeeping.
#[derive(Debug, Default)]
pub(super) struct Entry {
    pub(super) open: Option<OpenRound>,
    /// Which members closed this group's most recently *elected* round —
    /// the next round's own `awaiting` set, until it closes and updates this.
    ///
    /// ⚠️ **`None` for a group that has never closed one.** A group's true
    /// first-ever round has no prior membership to wait for, so it waits out
    /// its own deadline like every other
    /// unresolved round rather than closing the instant its own opener
    /// joins; real Kafka's `group.initial.rebalance.delay.ms` names the
    /// same gap for the same reason (batching concurrently-starting
    /// consumers into one round), and [`INITIAL_REBALANCE_DELAY`] is that
    /// gap here — bounded at 3 s rather than at `rebalance_timeout_ms`,
    /// which `M4.29` measured as five minutes of silence against a real
    /// client.
    pub(super) last_round_members: Option<Vec<String>>,
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

/// How long a round with no previous roster waits before closing on whoever
/// has arrived — Kafka's own `group.initial.rebalance.delay.ms`, which
/// defaults to the same 3 s.
///
/// ⚠️ **Without this a brand-new group holds its first joiner for the
/// client's whole `rebalance_timeout_ms`.** `awaiting` is seeded from the
/// previous roster, and a group nobody has joined before has none — so
/// nothing can empty it, nothing closes the round early, and it runs to its
/// deadline. Both librdkafka and the Java consumer seed
/// `rebalance_timeout_ms` from `max.poll.interval.ms`, **300 s** by default:
/// five minutes before a consumer receives its first partition. `M4.29`
/// measured it against real librdkafka rather than deriving it from the
/// defaults, which is what that row asked for.
///
/// ⚠️ **The same `None` covers a restarted broker.** `GroupJoins` is
/// per-node and in no log by design, so a restart loses `last_round_members`
/// and every group's next round looks brand new. One bound answers both, and
/// the defect was never restart-specific — which is how `M4.29` framed it
/// before the measurement.
///
/// ⚠️ **A floor on latency, so it is deliberately small.** Every group pays
/// it once per round that has no roster to wait for. Raising it delays every
/// first assignment; lowering it makes a fleet starting together more likely
/// to need a second rebalance, since members arriving after it has elapsed
/// join the next round rather than this one. 3 s is Kafka's own number for
/// that trade and there is no measurement here arguing for a different one.
///
/// ⚠️ **This window is fixed; Kafka's is re-armed, and that is a real
/// difference rather than a detail.** Kafka restarts the delay on each new
/// joiner, capped at the rebalance timeout, so a fleet arriving over 30 s
/// lands in *one* generation. Here the window runs from the round's install
/// and members arriving after it join the next round instead — the same 30 s
/// fleet costs roughly ten generations. For a *starting* fleet each is a
/// correct rebalance and no partition is double-assigned, so there it is a
/// churn cost rather than a safety one. Re-arming needs the round's deadline
/// to move after waiters have already captured it, which is a larger change
/// than this bound. Named by review rather than left for a reader to assume
/// parity.
///
/// ⚠️ **After a broker restart the shortened window is a real trade, not a
/// free one**, and the churn argument above does not cover it. A restarted
/// node has lost `last_round_members`, so it cannot tell "nobody has joined
/// yet" from "an incumbent has not reconnected yet". If one consumer returns
/// at once and another is still backing off — librdkafka's
/// `reconnect.backoff.max.ms` defaults to 10 s, longer than this delay — the
/// round closes on the first alone, and a cooperative assignor that sees no
/// other owner hands it a partition the absent member still holds. The
/// absent member is fenced `UNKNOWN_MEMBER_ID` on its first heartbeat or
/// `OffsetCommit` *after it reconnects* — not before, since both go to the
/// coordinator connection that is down — so the overlap is bounded by its
/// reconnect backoff plus one `heartbeat.interval.ms`: roughly 10 s with
/// librdkafka's defaults, not unbounded, but an overlap during which two
/// consumers can process the same partition. The 300 s deadline this
/// replaces always held the round open long enough. The trade is taken deliberately — five minutes of silence on
/// every restart is the worse failure — and the real fix is a roster the
/// restarted node can *read*, which needs the durable group metadata log
/// `roadmap.md` defers to `M6` (`ADR-0035`). Found by review.
pub(super) const INITIAL_REBALANCE_DELAY: Duration = Duration::from_secs(3);

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
    /// ⚠️ **Which members are still *live*, so a round does not wait for one
    /// that is never coming back.** `awaiting` is keyed on member ids, and a
    /// consumer that restarts or is fenced comes back with an empty
    /// `member_id` and is minted a *new* one — so its old id would sit in
    /// `awaiting` until the round's own deadline, stalling the group for a
    /// full `rebalance_timeout_ms` on each restart.
    ///
    /// ⚠️ **`is_live`, never `is_tracked`.** A `SIGKILL`ed consumer stays
    /// tracked indefinitely — nothing reaps in the background — so keying on
    /// tracking reinstates exactly that stall. Found by review.
    pub(crate) heartbeats: &'a crate::heartbeat::Heartbeats,
    /// ⚠️ **The sync barrier, because opening a round can strand the
    /// followers parked on the round before it.** `M4.43` gave the barrier a
    /// way to say a generation was refused and wired it to
    /// `submit_assignment`; `M4.47` wired it to every route a member is
    /// *lost* by. `MemberJoinedDuringSync` is the remaining route out of
    /// `CompletingRebalance`, and it is this module's: a newcomer's join
    /// reopens the barrier while the previous generation's leader is still
    /// computing, so the wake belongs wherever the barrier is reopened
    /// rather than wherever a member is removed. `M4.58`.
    pub(crate) sync_groups: &'a crate::sync_group::SyncGroups,
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

/// What re-pruning an open round's `awaiting` concluded — [`GroupJoins::reprune_awaiting`].
pub(crate) enum Reprune {
    /// Every member this round still waits for has died. The round can be
    /// closed now rather than at its own `rebalance_timeout_ms`.
    RosterDead,
    /// Still waiting, and this is the earliest instant the answer could
    /// change: the latest deadline among the members still awaited. A live
    /// member renews before then — `heartbeat::handle` renews even while
    /// answering `REBALANCE_IN_PROGRESS` — so waking here either finds it
    /// gone or pushes the next check out.
    Recheck(Instant),
    /// Nothing for this waiter to re-prune: not its round, already closed, or
    /// a round with no previous roster to wait for at all. ⚠️ **Not the same
    /// as `RosterDead`** — a brand-new group has no roster and must still
    /// wait out its deadline, and conflating the two closed every round on
    /// its first early wake.
    NotWaiting,
}
