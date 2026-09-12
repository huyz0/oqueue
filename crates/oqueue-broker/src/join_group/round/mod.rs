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
//! enrolled — never added to `members`, never counted toward `expected` —
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

    /// ⚠️ **Since `M4.15d`, every transition this path fires goes through
    /// the actor**, so `Join`, `MemberJoinedDuringSync` and
    /// `JoinBarrierComplete` are durably logged in the order they are
    /// applied, exactly as `M4.15c`'s own three call sites already were.
    /// Before this, they were applied to the live coordinator and never
    /// written down — which is why a restart rebuilt a group with no
    /// membership and why `GroupTransitionsTask::replay` had to poison
    /// essentially every real group it replayed.
    pub(crate) async fn join(
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
                let step = plan_join(
                    &mut entries,
                    co.coordinator,
                    group,
                    &member,
                    rebalance_timeout,
                );
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
        let applied = co.transitions.transition(group.clone(), event).await;
        let mut entries = self.lock();
        if applied.is_ok() {
            install_round(&mut entries, group, rebalance_timeout);
        }
        drop(entries);
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
            && Arc::ptr_eq(&round.outcome, own_outcome)
            && round.outcome.get().is_none()
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

    /// Closes the round while holding the slot, same discipline.
    async fn close_holding_slot(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
    ) -> Option<JoinOutcome> {
        let _slot = SlotGuard {
            joins: self,
            group: group.clone(),
        };
        self.apply_close(co, group).await
    }

    /// Fires `JoinBarrierComplete` with the lock released, then publishes the
    /// close to every waiter under it again.
    ///
    /// `None` means the barrier was refused: the round is **destroyed**
    /// (`abandon_round`) and the caller re-plans against the coordinator's real
    /// state. ⚠️ **Not "left open", which an earlier version of this sentence
    /// said and which is the wedge round three of this task's review found**:
    /// `plan_join` only consults the coordinator when it needs to *open* a
    /// round, so a round left open makes every re-plan re-fire the same illegal
    /// barrier until the process ends.
    ///
    /// ⚠️ **A refused barrier must not publish a close.** It used to fall
    /// back to the coordinator's *current* generation, so a round whose
    /// `JoinBarrierComplete` lost a race to an eviction's `AllMembersGone`
    /// answered every member `error_code: NONE` at a generation one behind,
    /// against a group the coordinator now calls `Empty` — a silently wrong
    /// success rather than something a client can act on. Found by review.
    async fn apply_close(&self, co: Coordination<'_>, group: &GroupId) -> Option<JoinOutcome> {
        let generation = close_generation(co, group).await;
        let mut entries = self.lock();
        // ⚠️ On a refusal the round is destroyed, not left open — see
        // `abandon_round`.
        let closed = match generation {
            Barrier::Closed(g) => finalize_close(&mut entries, group, g),
            Barrier::Illegal => {
                abandon_round(&mut entries, group, true);
                None
            }
            Barrier::Unavailable => {
                abandon_round(&mut entries, group, false);
                None
            }
        };
        drop(entries);
        closed.map(JoinOutcome::Ready)
    }

    /// Closes `group`'s own currently-open round if it has not already
    /// closed — a waiter's own deadline firing, `fetch::park`'s own
    /// deadline-wins-the-race shape. Idempotent: a round already closed
    /// (by reaching `expected`, or by a different waiter's own deadline)
    /// is left exactly as it was.
    ///
    /// ⚠️ **`own_outcome` is how a stale waiter is told its own round is
    /// gone, not the group's current one.** `entry.open` is *replaced* the
    /// moment a new round opens (`install_round`), keyed only by `GroupId` —
    /// so a waiter whose own round already closed *without* waking it (a
    /// `Notify::notify_waiters` call reaches only tasks already registered
    /// as waiters; one that has not yet reached its own `.notified().await`
    /// at that instant is not among them) would otherwise, on its own
    /// stale deadline, close whatever round is open *now* — a different
    /// group generation than the one this call ever joined. Comparing
    /// `own_outcome` against `entry.open`'s own `outcome` by pointer is
    /// what tells "my round, not yet closed" apart from "a round, but not
    /// mine": only the first may be closed here.
    /// ⚠️ **Since `M4.15d` this awaits the actor too**, so a round closed
    /// by a waiter's own deadline records its `JoinBarrierComplete`
    /// durably — the deadline path is how most real rounds close, so
    /// leaving it on the direct call would have left the common case
    /// undurable while the full-round path was fixed.
    pub(crate) async fn close_on_deadline(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
        own_outcome: &Arc<OnceLock<RoundOutcome>>,
    ) {
        // ⚠️ **A held slot means somebody else is already closing this round,
        // and this caller must wait for them rather than return.** Returning
        // looks safe — the holder publishes to `own_outcome`, which is what
        // the caller polls — but it is not: every waiter's deadline fires at
        // the same instant, so the two that lose the claim would report "no
        // close happened" while the winner is still awaiting the actor, and
        // `wait_for_close` gives up after its second attempt. Three members
        // joining one round is exactly that, and it is the shape of the test
        // that caught it.
        //
        // ⚠️ **Bounded, for the reason `join`'s own wait is**: a mutation
        // that made the slot look permanently held turned an unbounded wait
        // into a hang. A lapsed wait re-plans; the budget is what stops it
        // going round forever.
        // ⚠️ One budget here too, for `join`'s reason: a fresh `SLOT_WAIT`
        // per pass let two deadline attempts hold a connection for 80 s
        // against a client that may have asked for a zero-length round.
        let waiting_until = Instant::now() + SLOT_WAIT;
        let mut claimed = false;
        for _ in 0..MAX_JOIN_REPLANS {
            let waiting = {
                let mut entries = self.lock();
                let decided = match deadline_claim(&mut entries, group, own_outcome) {
                    DeadlineClaim::NotMine => return,
                    DeadlineClaim::Claimed => None,
                    DeadlineClaim::Wait(notify) => {
                        let mut notified = Box::pin(Arc::clone(&notify).notified_owned());
                        notified.as_mut().enable();
                        Some(notified)
                    }
                };
                drop(entries);
                decided
            };
            let Some(notified) = waiting else {
                claimed = true;
                break;
            };
            if tokio::time::timeout_at(waiting_until, notified)
                .await
                .is_err()
            {
                return;
            }
        }
        // ⚠️ **Exhausting the budget returns, it does not fall through.** An
        // earlier version left the loop's success path implicit, so
        // `MAX_JOIN_REPLANS` full passes ran on to close a round *without
        // holding the slot* — appending a second `JoinBarrierComplete` for a
        // round another task was already closing, bumping the generation past
        // the one every member had been answered with, and releasing that
        // other task's handle out from under it. Found by review. The flag is
        // set only where the claim is actually made.
        if !claimed {
            return;
        }

        let _slot = SlotGuard {
            joins: self,
            group: group.clone(),
        };
        let generation = close_generation(co, group).await;
        let mut entries = self.lock();
        match generation {
            Barrier::Closed(generation) => {
                finalize_close(&mut entries, group, generation);
            }
            Barrier::Illegal => abandon_round(&mut entries, group, true),
            Barrier::Unavailable => abandon_round(&mut entries, group, false),
        }
        drop(entries);
    }
}

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
pub(crate) use state::{Coordination, JoinOutcome, RoundClose, RoundMember, RoundOutcome};
use state::{Entry, MAX_JOIN_REPLANS, SLOT_WAIT, Step};

#[cfg(test)]
mod tests;
