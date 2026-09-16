//! Where a group's *sync* backstop comes from: the fold over the roster of the
//! round that is authoritative right now.
//!
//! ⚠️ **Split from `super` by `M4.83` for `code-structure.md` rule 16**, the
//! same seam `M4.15d` cut for `state.rs`: nothing here awaits anything, and
//! nothing here takes the rounds lock — `super` holds it across every call and
//! passes the map in. It is also the concept: a follower parking on the
//! assignment barrier cannot read `rebalance_timeout_ms` off its own
//! `SyncGroupRequest`, because the wire carries no timeout field there, so the
//! join side is what has to publish it.
//!
//! ⚠️ **This is where the rounds-then-barrier nesting is realised**, which is
//! worth saying here rather than only at the call sites: [`publish`] takes the
//! barrier's mutex while `super` holds the rounds mutex. A caller that already
//! holds the barrier's would self-deadlock on a non-reentrant lock —
//! `SyncGroups::submit`'s closure is the one place that could reach back this
//! way, and `barrier.rs` warns against it in its own terms. ⚠️ **It is not the
//! crate's only nesting**, which this file said twice until `M4.87`: `submit`
//! and `refuse` each hold `SyncGroups`'s `entries` while taking a group's
//! separate `Assignments` mutex. An author who believes there is one order
//! adds a path taking `entries` first and the rounds lock second, which is the
//! reverse of [`publish`] and deadlocks against it.
//!
//! ⚠️ **Two publishers, and the division is `M4.87`'s.** `GroupJoins::join`
//! publishes the *open* round's fold and nothing else; `close.rs` publishes the
//! closed roster under the same guard that took it. A version of `join` took
//! the outcome and folded `Ready`'s own `RoundClose` instead, because
//! `finalize_close` empties the roster with `mem::take` and a fold over the
//! open round then answers "nothing to say" — `M4.74`'s 6 s-then-300 s case in
//! a new shape. That arm is gone, and reinstating it is not a substitute for
//! either close publish: deleting `apply_close`'s fails
//! `a_member_whose_rejoin_closes_the_round_is_folded_in` at `Some(6s)`, and
//! deleting `close_on_deadline`'s leaves the suite green while reopening the
//! window a whole generation of followers waits in.
//!
//! ⚠️ **Derived, not accumulated, and that is the whole of `M4.83`.** The
//! previous writer folded `max` over every member that ever enrolled and
//! nothing lowered it, so a member that reached `Pending` asking the ceiling
//! and then disconnected held its group's sync backstop there for the life of
//! the process — cross-principal, since `GroupGrants` is deferred.

use super::state::{Entry, RoundClose, RoundMember};
use crate::sync_group::SyncGroups;
use oqueue_core::GroupId;
use std::collections::HashMap;
use tokio::time::Duration;

/// The largest `rebalance_timeout` a roster asks for, or `None` if it is empty.
pub(super) fn fold_timeouts(members: &[RoundMember]) -> Option<Duration> {
    members.iter().map(|m| m.rebalance_timeout).max()
}

/// Writes `group`'s sync backstop: the fold over `closed`'s members if a round
/// just closed, and over its open round's members otherwise.
///
/// ⚠️ **`closed` is how the round-closing member is counted at all**, and
/// `M4.74` is why that needs saying. `finalize_close` empties the roster with
/// `mem::take`, so the open round holds nobody once it has run and a fold over
/// it answers "nothing to say" — the member whose join closed the round would
/// never reach the barrier's number. `close.rs` publishes the close under the
/// same guard that took the roster; a fold over the open round alone was
/// measured at `Some(6s)` against `Some(300s)` on
/// `a_member_whose_rejoin_closes_the_round_is_folded_in`.
///
/// ⚠️ **`entries` is passed in rather than locked here**, so the read and the
/// write cannot be interleaved. Review measured the version that read under the
/// rounds lock and wrote after releasing it: a third member's `join` folds
/// `[greedy, b, c]` and blocks on the barrier's mutex, the greedy member's
/// `withdraw` folds `[b, c]` and wins it, and the join then writes the larger
/// value last — a departed member re-pinning the group, which across a
/// `close_on_deadline` is a whole generation of followers. The nesting is
/// rounds-then-barrier, so taking the second inside the first is legal;
/// `SyncGroups::submit`'s own closure-under-the-lock is the same shape for the
/// same reason, and is the crate's *other* nesting rather than a precedent for
/// this being the only one — see this module's own header.
///
/// ⚠️ **`None` means "do not answer", not "no timeout".** A round that has
/// closed still has followers at the sync barrier waiting on the number it
/// closed with, and [`crate::sync_group::deadline::sync_wait_ms`] answers
/// `None` with its ceiling — so [`SyncGroups::set_rebalance_timeout`] leaves
/// the stored value alone rather than clearing it. Overwriting here would put
/// every follower of a just-closed round back on the fifty minutes `M4.66` took
/// them off.
pub(super) fn publish(
    entries: &HashMap<GroupId, Entry>,
    sync_groups: &SyncGroups,
    group: &GroupId,
    closed: Option<&RoundClose>,
) {
    let fold = closed.map_or_else(
        || {
            entries
                .get(group)
                .and_then(|entry| entry.open.as_ref())
                .and_then(|round| fold_timeouts(&round.members))
        },
        |close| fold_timeouts(&close.members),
    );
    sync_groups.set_rebalance_timeout(group, fold);
}
