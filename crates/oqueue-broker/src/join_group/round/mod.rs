//! The join barrier: who is mid-round for a group, and when a round closes.
//!
//! ⚠️ **Its own module because this is state [`oqueue_core::GroupCoordinator`]
//! deliberately does not carry.** `M4.2`'s own trait holds
//! state/generation/assignment-epoch — the three values every implementation
//! must agree on, sans-I/O (`ADR-0034`). Who is mid-`JoinGroup`, how many
//! members a round expects, and the deadline it closes by are this broker's
//! own bookkeeping: async, per-node, and never durable, the same split
//! `fetch::park` draws between `IndexWatch` (the coordinator's own) and the
//! parking loop around it (the broker's own).
//!
//! ⚠️ **Members are admitted one at a time, by re-running `M4.6`'s own
//! `elect`.** A member whose addition would leave the round's own running
//! candidate set empty is refused `INCONSISTENT_GROUP_PROTOCOL` and never
//! enrolled — never added to `members`, never waited for by the next round —
//! rather than being collected and only checked once the round closes. This
//! is what makes the round's own eventual `elect` call at close time
//! infallible: every enrolled member is, by construction, pairwise
//! compatible with every other.
//!
//! ⚠️ **`Dead` is unreachable through this path in this milestone.**
//! [`oqueue_core::GroupEvent::Expire`] has no caller anywhere in this
//! workspace (`group_state.rs`'s own module doc: "no `DeleteGroups`-style
//! admin API exists in this milestone's own scope") — a group this broker's
//! `GroupCoordinator` implementation ever hands out only ever reaches
//! `Dead` from a caller `M12`'s own future scope adds. Reaching it here
//! would mean this bookkeeping and the coordinator's own state have
//! diverged, a bug rather than a case a client can trigger — refused
//! rather than panicked on, `security.md` rule 3's instinct applied to an
//! internal invariant instead of untrusted wire bytes.

use oqueue_core::{GroupEvent, GroupId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tokio::time::{Duration, Instant};

/// Mints a fresh member id — process-unique, [`crate::writer_id::WriterId`]'s
/// own `MINTED`-counter idiom, `member_id.rs`'s own "the broker's own
/// minting scheme never produces an empty one" contract.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// A fresh member id no client chose. Infallible in practice — `"member-"`
/// is a non-empty literal prefix, so the wrapped string can never be empty —
/// but [`oqueue_core::MemberId::new`] still returns a `Result`, and
/// `code-structure.md` rule 26 bans `.unwrap()`/`.expect()` regardless of
/// how certain that is (`init_producer_id.rs`'s own precedent for a
/// provably-unreachable error arm).
pub(crate) fn mint_member_id() -> oqueue_core::MemberId {
    let n = MINTED.fetch_add(1, Ordering::Relaxed);
    oqueue_core::MemberId::new(format!("member-{n}"))
        .unwrap_or_else(|_| unreachable!("\"member-{n}\" always starts with a non-empty literal"))
}

/// `rebalance_timeout_ms` is absent below `JoinGroup` v1 and decodes to `0`
/// (`oqueue_codec::join_group`'s own doc) — `M4.7`'s own decision named
/// there as this task's to make: an absent value falls back to
/// `session_timeout_ms`, matching real Kafka's own v0 behaviour (the
/// rebalance timeout and the session timeout were the same wire field
/// before `KIP-62` split them).
pub(crate) const fn effective_timeout_ms(
    rebalance_timeout_ms: i32,
    session_timeout_ms: i32,
) -> i32 {
    if rebalance_timeout_ms > 0 {
        rebalance_timeout_ms
    } else {
        session_timeout_ms
    }
}

/// Every group's own join-round state, live for as long as this node is.
#[derive(Debug, Default)]
pub(crate) struct GroupJoins {
    entries: Mutex<HashMap<GroupId, Entry>>,
}

impl GroupJoins {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<GroupId, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Joins `member` to `group`'s current round, and records what it asked
    /// for so the *sync* half can be bounded by the same number.
    ///
    /// ⚠️ **Since `M4.15d`, every transition this path fires goes through
    /// the actor**, so `Join`, `MemberJoinedDuringSync` and
    /// `JoinBarrierComplete` are durably logged in the order they are
    /// applied, exactly as `M4.15c`'s own three call sites already were.
    /// Before this, they were applied to the live coordinator and never
    /// written down — which is why a restart rebuilt a group with no
    /// membership and why `GroupTransitionsTask::replay` had to poison
    /// essentially every real group it replayed.
    ///
    /// ⚠️ **The sync half is bounded by this number, and until `M4.66` it was
    /// bounded by a constant ten times it.** A follower parking on the
    /// assignment barrier cannot read `rebalance_timeout_ms` off its own
    /// `SyncGroupRequest` — the wire carries no timeout field there — so the
    /// members that join are what record it, folded to the maximum
    /// (`SyncGroups::note_rebalance_timeout`'s own doc).
    ///
    /// ⚠️ **Only a member that was actually enrolled**, and review measured
    /// why: the first version recorded every attempt, so one request refused
    /// for an unusable protocol while asking `rebalance_timeout_ms=3_000_000`
    /// left the group at the ceiling for the rest of the process —
    /// `waited_ms=3000000`, the `M4.47` stranding `M4.66` exists to end, set
    /// by a member that never joined.
    ///
    /// ⚠️ **Three outcomes are excluded, and the third is the one review had
    /// to measure.** [`JoinOutcome::Refused`]'s own doc is "this member was
    /// never enrolled" and [`JoinOutcome::Unavailable`] is a transition that
    /// did not land — both obvious. [`JoinOutcome::Busy`] is exhaustion, and
    /// every pass that can precede it leaves the member unenrolled:
    /// `Step::Wait` and `Step::Open` never reach `plan_join`'s enrolment, and
    /// a `Step::Close` that returns `None` means `apply_close` called
    /// `abandon_round`, which takes `entry.open` — the enrolment dies with the
    /// round. A first version of this predicate omitted it, and an exhausted
    /// join asking for fifty minutes then held the whole group at the ceiling
    /// for the life of the process, because the fold is monotonic. ⚠️ **The
    /// sentence here said "everything else is in the round"**, which is the
    /// same untrue-comment class this row was opened for.
    ///
    /// ⚠️ **One exit point, and `M4.74` is why it is a wrapper rather than a
    /// line in the loop.** The recording sat in the `Step::Done` arm and
    /// matched `Ready | Pending` — but every `Step::Done` in this module
    /// carries `Refused` or `Pending`, and `Ready` comes back from the
    /// `Step::Close` arm, which returned around it. So the arm matched a
    /// variant it could never see while the round-*closing* member, which is
    /// enrolled, was never folded in: a member asking 6 s then rejoining and
    /// asking 300 s closed the round and left the group at 6 s. Asking the
    /// question once, where the outcome is whatever the loop produced, is what
    /// no further arm can be added around.
    pub(crate) async fn join(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
        member: RoundMember,
        rebalance_timeout: Duration,
    ) -> JoinOutcome {
        let outcome = self
            .plan_and_join(co, group, member, rebalance_timeout)
            .await;
        if !matches!(
            outcome,
            JoinOutcome::Refused | JoinOutcome::Unavailable | JoinOutcome::Busy
        ) {
            // ⚠️ Outside any lock this module holds: the only order in the
            // crate is rounds-then-barrier, and holding neither across the
            // other is cheaper than defending a nesting
            // (`async-concurrency.md` rule 9).
            co.sync_groups
                .note_rebalance_timeout(group, rebalance_timeout);
        }
        outcome
    }

    /// The re-planning loop itself: read the group's state, choose a step,
    /// apply it, and start again if another task got there first —
    /// [`MAX_JOIN_REPLANS`] times, inside one [`SLOT_WAIT`] budget.
    ///
    /// ⚠️ **Separated from [`Self::join`] by `M4.74`, and not for tidiness.**
    /// The recording that wrapper does used to live in this loop's
    /// `Step::Done` arm, where it could not see the outcome the `Step::Close`
    /// arm returns around it. A loop with several exits cannot carry a
    /// question that has to be asked of every exit; a wrapper can.
    async fn plan_and_join(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
        member: RoundMember,
        rebalance_timeout: Duration,
    ) -> JoinOutcome {
        // ⚠️ **One budget for all the waiting, not one per pass.** Each
        // `Step::Wait` used to absorb a fresh `SLOT_WAIT`, so eight passes
        // could hold a connection's own in-flight permit for 40 s — and
        // `wait_for_close`'s two deadline passes add two more, ~50 s, for a
        // client that may have asked for a zero-length round. The budget is
        // the liveness bound; the replan count is separate and bounds work,
        // not time. Found by review.
        let waiting_until = Instant::now() + SLOT_WAIT;
        for _ in 0..MAX_JOIN_REPLANS {
            // ⚠️ **The guard's scope is this block, and nothing else.** A
            // `MutexGuard` is not `Send`, so a future holding one across an
            // `.await` is not `Send` either — which `dispatch.rs`'s own
            // `impl Future + Send` bound turns into a compile error rather
            // than a latent bug. That is `async-concurrency.md` rule 6 being
            // enforced by the type system, and it is why the plan is
            // computed and the waiter registered in one scoped block whose
            // value is carried out, rather than with a `drop` the analysis
            // has to be trusted to honour.
            let (step, waiting) = {
                let mut entries = self.lock();
                let step = plan_join(&mut entries, co, group, &member, rebalance_timeout);
                // ⚠️ Registered here, under the guard — see `claim_or_wait`.
                let waiting = match &step {
                    Step::Wait(notify) => {
                        let mut notified = Box::pin(Arc::clone(notify).notified_owned());
                        notified.as_mut().enable();
                        Some(notified)
                    }
                    _ => None,
                };
                drop(entries);
                (step, waiting)
            };

            match step {
                Step::Done(outcome) => return outcome,
                Step::Wait(_) => {
                    if let Some(notified) = waiting {
                        // ⚠️ **Bounded, and a mutation test is why.** Mutating
                        // `claim_or_wait` to always report the slot held made
                        // this wait never return — `cargo mutants` reported a
                        // TIMEOUT rather than a killed mutant, which is the
                        // tool saying the code can hang rather than that the
                        // test is weak. Nothing should wait on this longer
                        // than the join it belongs to would have waited
                        // anyway, and a lapsed wait simply re-plans: the loop
                        // re-reads the state and decides again, so a missed
                        // wakeup costs a pass of the budget instead of the
                        // request. `fetch::park`'s own deadline discipline.
                        // ⚠️ `SLOT_WAIT`, not `rebalance_timeout`: this is a liveness
                        // bound on *this node's* bookkeeping, not a promise to
                        // the client. `deadline.rs` clamps a non-positive
                        // client timeout to zero, which turned a concurrent
                        // join into eight zero-length waits and a fatal
                        // `INCONSISTENT_GROUP_PROTOCOL`. Found by review.
                        let _ = tokio::time::timeout_at(waiting_until, notified).await;
                    }
                }
                // ⚠️ **A refused event re-plans; it does not refuse the
                // join.** `opening_event` chooses from the state it reads
                // under the lock, and the actor applies it a durable
                // object-store append later — so an ordinary
                // `SyncComplete` or eviction landing in between makes the
                // chosen event illegal without anything being wrong. That
                // is the *normal* end of a rebalance, not a divergence.
                // Returning `Refused` here answered it with
                // `INCONSISTENT_GROUP_PROTOCOL`, which the Java consumer
                // treats as fatal and leaves the group over. Re-planning
                // re-reads the state and picks the event that is legal now.
                // Found by review; before `M4.15d` the window was two
                // adjacent statements under one mutex, and widening it is
                // what made this reachable.
                //
                // ⚠️ A genuinely refusable group still refuses, and fast:
                // `Dead` fails `opening_event` on the very next pass, which
                // is a plan-time `Step::Done(Refused)` rather than eight
                // round trips.
                Step::Open(event) => {
                    if let Some(answer) = self
                        .open_holding_slot(co, group, event, rebalance_timeout)
                        .await
                    {
                        return answer;
                    }
                }
                Step::Close => {
                    if let Some(answer) = self.close_holding_slot(co, group).await {
                        return answer;
                    }
                }
            }
        }
        // ⚠️ **Exhaustion is `Busy`, never `Refused`.** It means the group
        // is contended or its actor is slow, not that anything about this
        // request is wrong — and `Refused` maps to
        // `INCONSISTENT_GROUP_PROTOCOL`, which the Java consumer raises out
        // of `poll()` and never retries. The shared wait budget makes this
        // easy to reach: once it lapses, every remaining pass returns at
        // once, so a second joiner arriving during an ordinary latency spike
        // burns all eight in a moment.
        JoinOutcome::Busy
    }

    /// Applies a round-opening transition with the lock released, then
    /// installs the round under it again. ⚠️ The three-way answer is the
    /// point: `Raced` means another handler's transition got there first and
    /// the caller should re-plan, while `Unavailable` means a dependency
    /// failed and re-planning will not help — conflating them answered a
    /// storage outage with a refusal the Java consumer never retries.
    ///
    /// ⚠️ **This function does not touch the slot.** Its caller
    /// [`Self::open_holding_slot`] owns the claim through a `SlotGuard`, so
    /// the release happens on every path including an unwind — adding an
    /// explicit release here would double-release and hand the slot to a
    /// waiter the guard is about to take it back from.
    async fn apply_open(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
        event: GroupEvent,
        rebalance_timeout: Duration,
    ) -> Applied {
        let reopens_the_barrier = matches!(event, GroupEvent::MemberJoinedDuringSync);
        let applied = co.transitions.transition(group.clone(), event).await;
        let mut entries = self.lock();
        if applied.is_ok() {
            install_round(&mut entries, group, rebalance_timeout);
        }
        drop(entries);
        // ⚠️ **A newcomer's join strands the previous generation's followers
        // unless something tells them, and this is the route `M4.47` does
        // not cover.** That row wired the barrier's refusal into
        // `Heartbeats::remove_where`, which is every way a member is *lost*;
        // `MemberJoinedDuringSync` is the way a round is *opened* on top of
        // one still syncing, and the leader computing that generation's
        // assignment will never submit it. Without this the follower waits
        // on the next round's join deadline rather than on
        // `MAX_SYNC_WAIT_MS`, which is why `M4.58` is its own row and not a
        // blocking finding against `M4.47` — lesser, and the same class.
        //
        // ⚠️ **The generation is the interrupted one, not a new one.**
        // `group_state.rs` advances the generation only at
        // `JoinBarrierComplete`, so `(CompletingRebalance,
        // MemberJoinedDuringSync) -> PreparingRebalance` carries it
        // unchanged and the record read back names exactly the generation
        // whose followers are parked.
        //
        // ⚠️ **Only on `Ok`, and here that is right** — unlike
        // `remove_where`, where `members.retain` drops the member before the
        // enqueue so a refused transition strands it anyway (`M4.47`'s own
        // second test). Nothing is mutated before this transition: a refused
        // or unavailable one leaves the group in `CompletingRebalance` with
        // its leader still able to submit, and refusing the barrier would
        // strand followers whose assignment is still coming.
        if reopens_the_barrier && let Ok(record) = &applied {
            co.sync_groups.refuse(group, record.generation.get());
        }
        match applied {
            Ok(_) => Applied::Installed,
            // ⚠️ **An illegal transition is a lost race; anything else is a
            // broken dependency, and the two must not be conflated.**
            // `GroupTransitions::transition` fails three ways: the event was
            // illegal (another handler got there first — re-plan), the serve
            // task is gone, or the durable append failed because the log is
            // unreachable. The last two do not get better by re-planning, and
            // answering them `INCONSISTENT_GROUP_PROTOCOL` after eight
            // attempts kills every consumer in the group permanently — a
            // storage outage should not outlive itself. Found by review;
            // before `M4.15d` this path called the pure in-memory
            // coordinator, whose only failure was `Dead`.
            Err(oqueue_core::Error::IllegalGroupTransition { .. }) => Applied::Raced,
            Err(_) => Applied::Unavailable,
        }
    }

    /// Removes `member_id` from `group`'s own open round, if it is still in
    /// one.
    ///
    /// ⚠️ **A member told to rejoin must not stay in the roster it was told to
    /// leave.** `plan_join` enrols before returning `Pending`, so a joiner
    /// whose `wait_for_close` gives up — both deadline passes lapsing against
    /// a slot somebody else holds — was answered `REBALANCE_IN_PROGRESS` while
    /// still counted in `round.members`. The round then closed with it in the
    /// roster, the leader assigned it partitions, and nothing consumed them:
    /// the client had already rejoined under a freshly minted id, and the
    /// abandoned one was never passed to `register_heartbeat`, so the eviction
    /// sweep could not remove it either. Found by review.
    ///
    /// ⚠️ The *abandonment* path needs no withdrawal — `abandon_round` takes
    /// the whole round, so the enrolment dies with it.
    /// ⚠️ **`own_outcome` identifies the round, exactly as it does for
    /// `deadline_claim`.** Without it this removed `member_id` from whatever
    /// round happened to be open: a client that times out and reconnects has
    /// *two* handlers for one member id, and the first one giving up would
    /// delete the second one's live enrolment — the round then closes with the
    /// member absent from the roster the leader is handed, while that second
    /// handler reads the same close, answers `NONE`, and registers for
    /// heartbeats. A tracked, unfenced member with no assignment, consuming
    /// nothing and evicted by nothing. Found by review.
    pub(crate) fn withdraw(
        &self,
        group: &GroupId,
        member_id: &str,
        own_outcome: &Arc<OnceLock<RoundOutcome>>,
    ) {
        let mut entries = self.lock();
        if let Some(entry) = entries.get_mut(group)
            && let Some(round) = entry.open.as_mut()
            && plan::is_own_open_round(round, own_outcome)
        {
            round.members.retain(|m| m.member_id != member_id);
        }
        drop(entries);
    }

    /// Applies a round-opening transition while holding `group`'s own
    /// in-flight slot. `None` means "re-plan"; `Some` is the caller's answer.
    ///
    /// ⚠️ The slot is owned by a [`SlotGuard`] here rather than released on
    /// each path, so an unwind cannot leak it — see `release`.
    async fn open_holding_slot(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
        event: GroupEvent,
        rebalance_timeout: Duration,
    ) -> Option<JoinOutcome> {
        let _slot = SlotGuard {
            joins: self,
            group: group.clone(),
        };
        match self.apply_open(co, group, event, rebalance_timeout).await {
            Applied::Installed | Applied::Raced => None,
            Applied::Unavailable => Some(JoinOutcome::Unavailable),
        }
    }
}

mod close;
mod plan;
mod slot;
mod state;

/// What one attempt to apply a round-opening transition did.
enum Applied {
    /// The event landed and the round is installed.
    Installed,
    /// The event was refused as illegal — another handler's transition got
    /// there first. Re-plan against the state it left.
    Raced,
    /// A dependency failed. Re-planning will not help.
    Unavailable,
}

use plan::{
    Barrier, DeadlineClaim, abandon_round, close_generation, deadline_claim, finalize_close,
    install_round, plan_join,
};
use slot::SlotGuard;
pub(crate) use state::{Coordination, JoinOutcome, Reprune, RoundClose, RoundMember, RoundOutcome};
use state::{Entry, MAX_JOIN_REPLANS, SLOT_WAIT, Step};

#[cfg(test)]
mod tests;
