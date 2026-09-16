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
//! ⚠️ **This is where the one lock nesting in the crate is realised**, which
//! is worth saying here rather than only at the call sites: [`publish`] takes
//! the barrier's mutex while `super` holds the rounds mutex. A caller that
//! already holds the barrier's would self-deadlock on a non-reentrant lock —
//! `SyncGroups::submit`'s closure is the one place that could reach back this
//! way, and `barrier.rs` warns against it in its own terms.
//!
//! ⚠️ **Derived, not accumulated, and that is the whole of `M4.83`.** The
//! previous writer folded `max` over every member that ever enrolled and
//! nothing lowered it, so a member that reached `Pending` asking the ceiling
//! and then disconnected held its group's sync backstop there for the life of
//! the process — cross-principal, since `GroupGrants` is deferred.

use super::state::{Entry, JoinOutcome, RoundClose, RoundMember};
use crate::sync_group::SyncGroups;
use oqueue_core::GroupId;
use std::collections::HashMap;
use tokio::time::Duration;

/// The largest `rebalance_timeout` a roster asks for, or `None` if it is empty.
pub(super) fn fold_timeouts(members: &[RoundMember]) -> Option<Duration> {
    members.iter().map(|m| m.rebalance_timeout).max()
}

/// The roster a just-closed round settled on, if this outcome is one.
///
/// ⚠️ **`Ready` has to fold over the close and not over the open round, and
/// that is `M4.74`'s finding in a new shape.** A join that *closes* a round
/// leaves `entry.open` as `None`, so asking for the open round's roster
/// answers "nothing to say" and the round-closing member is never counted:
/// measured at `Some(6s)` against `Some(300s)` on
/// `a_member_whose_rejoin_closes_the_round_is_folded_in`, which is the exact
/// case `M4.74` filed, restored by the repair for a different defect.
pub(super) fn ready_close(outcome: &JoinOutcome) -> Option<&RoundClose> {
    match outcome {
        JoinOutcome::Ready(close) => Some(close),
        _ => None,
    }
}

/// Writes `group`'s sync backstop: the fold over `closed`'s members if a round
/// just closed, and over its open round's members otherwise.
///
/// ⚠️ **`entries` is passed in rather than locked here**, so the read and the
/// write cannot be interleaved. Review measured the version that read under the
/// rounds lock and wrote after releasing it: a third member's `join` folds
/// `[greedy, b, c]` and blocks on the barrier's mutex, the greedy member's
/// `withdraw` folds `[b, c]` and wins it, and the join then writes the larger
/// value last — a departed member re-pinning the group, which across a
/// `close_on_deadline` is a whole generation of followers. The nesting is
/// rounds-then-barrier, the only order in this crate, so taking the second
/// inside the first is legal; `SyncGroups::submit`'s own closure-under-the-lock
/// is the same shape for the same reason.
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
