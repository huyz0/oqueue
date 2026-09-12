//! The per-group in-flight slot: who is mid-transition for a group, and how
//! that claim is given back.
//!
//! ⚠️ **Its own module because the release is the load-bearing half.**
//! Routing the round's transitions through the actor made two of them `async`,
//! so the decision and its application stopped being one step under one mutex
//! — the slot is what makes the pair atomic again. A slot left claimed wedges
//! every later join for that group for the life of the process, which is why
//! the release is owned by a `Drop` rather than by a call on each path: this
//! workspace is `panic = "unwind"` so a handler panic kills one connection
//! rather than the node, and `GroupJoins::lock` recovers from poisoning, so a
//! panic between the claim and the release used to leak it silently.

use super::{Entry, GroupJoins};
use oqueue_core::GroupId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Notify;

impl GroupJoins {
    /// Claims `group`'s own in-flight slot, or hands back the `Notify` of
    /// whoever already holds it.
    ///
    /// ⚠️ **The caller must register on the returned `Notify` before it drops
    /// the guard, and this function does not do that for it.**
    /// `Notify::notified()` registers the waiter when the future is first
    /// *polled*, not when it is created — so releasing the lock and only then
    /// awaiting leaves a window in which the holder's `notify_waiters()`
    /// reaches nobody and the task waits out its whole budget for a wakeup
    /// that already happened. Both call sites therefore do
    /// `Box::pin(notify.notified())` followed by `as_mut().enable()` while
    /// still holding the guard — `tokio`'s own documented remedy. ⚠️ An
    /// earlier version of this paragraph claimed the registration happened
    /// here; it does not, and a third caller written on the strength of that
    /// claim would have had exactly the hang described above.
    pub(super) fn claim_or_wait(
        entries: &mut HashMap<GroupId, Entry>,
        group: &GroupId,
    ) -> Option<Arc<Notify>> {
        let entry = entries.entry(group.clone()).or_default();
        if let Some(held) = entry.in_flight.as_ref() {
            return Some(Arc::clone(held));
        }
        entry.in_flight = Some(Arc::new(Notify::new()));
        None
    }

    /// Releases `group`'s own in-flight slot and wakes everyone waiting on
    /// it.
    ///
    /// ⚠️ **Call this through [`SlotGuard`], not directly.** A slot left
    /// claimed wedges every later join for that group until the process ends,
    /// and "every path releases it" is not a property `return` statements can
    /// hold: this workspace is `panic = "unwind"` precisely so a handler panic
    /// kills one connection rather than the node, and `lock` deliberately
    /// recovers from poisoning — so a panic anywhere between the claim and the
    /// release used to leak the slot permanently. Before `M4.15d` the critical
    /// section was a plain `MutexGuard` and a panic left the map usable. Found
    /// by review.
    pub(super) fn release(entries: &mut HashMap<GroupId, Entry>, group: &GroupId) {
        if let Some(entry) = entries.get_mut(group)
            && let Some(notify) = entry.in_flight.take()
        {
            notify.notify_waiters();
        }
    }
}

/// Holds `group`'s own in-flight slot until dropped — including by an unwind.
///
/// ⚠️ **The claim itself is made under the lock, by `claim_or_wait`; this only
/// owns the *release*.** Constructing one for a claim that was not made would
/// release somebody else's, so it is built exactly where a `Step` or a
/// `DeadlineClaim` reports the claim succeeded.
pub(super) struct SlotGuard<'a> {
    pub(super) joins: &'a GroupJoins,
    pub(super) group: GroupId,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let mut entries = self.joins.lock();
        GroupJoins::release(&mut entries, &self.group);
        drop(entries);
    }
}
