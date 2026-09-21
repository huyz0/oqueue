//! Closing a round: the deadline path, the dead-roster path, and the
//! transition that publishes the result.
//!
//! ⚠️ **Split out of `round/mod.rs` by concept, not by size.** `mod.rs` owns
//! getting a member *into* a round — planning, enrolling, opening. This file
//! owns getting the round *out*: what closes it, when, and who is allowed to.
//! The two halves share only `GroupJoins` and the slot discipline.

use super::{
    Barrier, Coordination, DeadlineClaim, GroupJoins, JoinOutcome, MAX_JOIN_REPLANS, Reprune,
    RoundClose, RoundOutcome, SLOT_WAIT, SlotGuard, abandon_round, close_generation,
    deadline_claim, finalize_close, plan, sync_wait,
};
use oqueue_core::{GroupId, GroupRosterSnapshot};
use std::sync::{Arc, OnceLock};
use tokio::time::Instant;

impl GroupJoins {
    /// Closes the round while holding the slot, same discipline.
    pub(super) async fn close_holding_slot(
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
    pub(super) async fn apply_close(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
    ) -> Option<JoinOutcome> {
        let generation = close_generation(co, group).await;
        let closed = {
            let mut entries = self.lock();
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
            if let Some(close) = &closed {
                sync_wait::publish(&entries, co.sync_groups, group, Some(close));
            }
            drop(entries);
            closed
        };
        if let Some(close) = &closed
            && !persist_roster(co.transitions, group, Some(close)).await
        {
            return Some(JoinOutcome::Unavailable);
        }
        closed.map(JoinOutcome::Ready)
    }

    async fn finish_deadline_close(&self, co: Coordination<'_>, group: &GroupId) {
        let generation = close_generation(co, group).await;
        let closed = {
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
            // ⚠️ **The closer publishes, and `M4.87` is why.** `finalize_close`
            // empties the roster with `mem::take`, so after this point the round
            // is installed and holds nobody — and a join that enrolled a moment
            // ago, dropped the rounds lock, and is on its way to publishing finds
            // nothing left to fold and writes nothing. Its ask is lost and the
            // generation closes on whatever an earlier, smaller member published.
            // Publishing here, under the same guard that took the roster, is what
            // makes that unreachable rather than unlikely.
            if let Some(close) = &closed {
                sync_wait::publish(&entries, co.sync_groups, group, Some(close));
            }
            drop(entries);
            closed
        };
        let _ = persist_roster(co.transitions, group, closed.as_ref()).await;
    }

    /// Re-prunes `awaiting` for the round `own_outcome` belongs to, and says
    /// what waiting for it is now worth. See [`Reprune`].
    ///
    /// ⚠️ **This is the only place a *dead* id is struck off `awaiting`.**
    /// `plan_join` strikes off the enrolling member's own id and nothing else, so
    /// without this a round would wait for every member of the previous roster
    /// until its own `rebalance_timeout_ms` — 300 s with the Java consumer's
    /// defaults — including members that are never coming. The common case is a
    /// consumer restarting *inside* its own session window: it returns under a
    /// new minted id while the old one is still tracked and still live when the
    /// round is installed. The count-based rule this replaced closed such a round
    /// at once, which makes the stall one the roster rule introduced and has to
    /// answer for itself.
    ///
    /// ⚠️ **Liveness was once tested here *and* at install *and* on every
    /// enrolment**, and review showed the other two could each be deleted with
    /// the whole suite still green, because this one reaches the same state a hop
    /// later. They are gone: a branch nothing can pin is a branch nothing is
    /// checking. Do not add them back. Found by review.
    pub(crate) fn reprune_awaiting(
        &self,
        co: Coordination<'_>,
        group: &GroupId,
        own_outcome: &Arc<OnceLock<RoundOutcome>>,
    ) -> Reprune {
        let mut entries = self.lock();
        // ⚠️ Three distinct "nothing to do here" cases, and they are *not*
        // the same as an empty roster. A round with no previous roster to
        // wait for (`awaiting == None`, a brand-new group) must wait out its
        // deadline like it always did; only a roster that *had* members and
        // has lost every one of them is closable early. Collapsing these into
        // one `Option` is what made an early wake close every round it woke
        // on, including one whose awaited members were alive and heartbeating.
        let Some(entry) = entries.get_mut(group) else {
            return Reprune::NotWaiting;
        };
        let Some(round) = entry.open.as_mut() else {
            return Reprune::NotWaiting;
        };
        if !plan::is_own_open_round(round, own_outcome) {
            return Reprune::NotWaiting;
        }
        let Some(awaiting) = round.awaiting.as_mut() else {
            return Reprune::NotWaiting;
        };
        awaiting.retain(|id| co.heartbeats.is_live(group, id));
        let next = co.heartbeats.latest_deadline(group, awaiting);
        drop(entries);
        next.map_or(Reprune::RosterDead, Reprune::Recheck)
    }

    /// Closes `group`'s own currently-open round if it has not already
    /// closed — a waiter's own deadline firing, `fetch::park`'s own
    /// deadline-wins-the-race shape. Idempotent: a round already closed
    /// (by its roster completing, or by a different waiter's own deadline)
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
        self.finish_deadline_close(co, group).await;
    }
}

async fn persist_roster(
    transitions: &crate::group_transitions::GroupTransitions,
    group: &GroupId,
    close: Option<&Arc<RoundClose>>,
) -> bool {
    let Some(close) = close else {
        return true;
    };
    transitions
        .set_roster(group.clone(), roster_snapshot(close))
        .await
        .is_ok()
}

fn roster_snapshot(close: &RoundClose) -> GroupRosterSnapshot {
    GroupRosterSnapshot {
        protocol_type: close.protocol_type.clone(),
        protocol_name: close.protocol_name.clone(),
        member_ids: close
            .members
            .iter()
            .map(|member| member.member_id.clone())
            .collect(),
    }
}
