//! The barrier a generation's assignment is published on, and awaited at.
//!
//! ⚠️ **Its own file because `sync_group.rs` reached `code-structure.md`
//! rule 16's 500-line limit** when `M4.43` gave the barrier a way to say a
//! generation was refused. The cut is along the seam the module already
//! had: this file is the shared cell and everything that reads or writes
//! it, and `sync_group.rs` is the handler that decodes a request and
//! decides which of those to call. `thing.rs` beside `thing/` rather than
//! a `mod.rs` — rule 8.

use oqueue_core::GroupId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;

/// Every member's own slice of the leader's submitted assignment, by member
/// id, together with **the generation it was computed for**.
///
/// ⚠️ **The generation is the whole point, and leaving it out was a real
/// defect `M4.17` caught with a real client.** A follower syncing for
/// generation N would find generation N-1's map already present and be
/// answered from it at once, without waiting for its own generation's
/// leader to submit. For a consumer that joined during N-1 that map holds
/// its own *empty* slice — it was the newcomer, and a cooperative assignor
/// gives a newcomer nothing in the round that admits it — so the consumer
/// was answered "you own nothing", went `steady`, and stayed idle forever
/// while the rest of the group kept every partition. Adding a consumer to a
/// working group simply did not work.
///
/// ⚠️ **`pub(super)` on the struct, private on its fields**, and the
/// asymmetry is deliberate: the type leaks through `Assignments` in
/// `entry_for`'s and `assignment_for`'s signatures, so it has to be
/// nameable — but `entry_for` hands out the cell's own handle, and exported
/// fields would let anything in this module tree write an `Assignment`
/// directly and pin a state neither writer can produce, bypassing both
/// `submit`'s generation guard (`M4.35`) and `refuse`'s (`M4.43`). Found by
/// review, which measured that the fields can be private while the struct
/// cannot.
///
/// ⚠️ **`#[derive]` goes below the whole doc comment, not between its two
/// halves.** It sat between them until `M4.55`, which rustdoc renders as one
/// run-on paragraph — `M4.17`'s generation argument above and this
/// visibility argument glued together, so a reader cannot tell which claim
/// the emphasis belongs to. Recorded by `M4.36`'s commit body and
/// unharvested until the sweep.
#[derive(Debug)]
pub(super) struct Assignment {
    generation: i32,
    /// `None` means this generation was *refused* — its leader's submission
    /// did not land and never will. ⚠️ **`M4.43`: the barrier had no way to
    /// say that**, so a follower already parked when the leader was refused
    /// waited out `MAX_SYNC_WAIT_MS` for an assignment nobody would submit.
    /// It is normally freed as [`Known::Superseded`] by the next
    /// generation's submission — but the refusal this was built for is an
    /// unreachable group metadata log, and every path to a next generation
    /// goes through the same log, so while the outage lasts there is no
    /// next generation to free it.
    map: Option<Arc<HashMap<String, Vec<u8>>>>,
}

/// The cell a group's current [`Assignment`] lives in.
///
/// ⚠️ **A `Mutex<Option<_>>`, not a `OnceLock`.** Each generation replaces
/// the last, and the previous shape could only ever be *rotated* — replacing
/// the whole entry, and with it the `Notify` any follower was already parked
/// on, so a follower that arrived before its leader was left waiting on a
/// handle nobody would ever signal and timed out. One durable cell and one
/// durable `Notify` per group means a waiter is woken by every submission
/// and re-checks the generation itself.
///
/// ⚠️ **A newtype, not the bare `Arc<Mutex<Option<_>>>` it wraps**, and what
/// the wrapper buys is a write nobody can make from outside this file.
/// `entry_for` hands this handle to `sync_group.rs` and to every test in the
/// module tree; as a plain alias, `*handle.lock().unwrap() = None` erased a
/// published map **with no notify** and returned that generation's followers
/// to [`Known::Waiting`] — the stranding `M4.43` exists to end, reachable
/// from a caller that only meant to read, and bypassing both `submit`'s
/// generation guard (`M4.35`) and `refuse`'s (`M4.43`). The same asymmetry
/// `Assignment`'s own fields already had, one level out; found by M4's final
/// boundary review, and `M4.57` is the row.
///
/// Mutation is `SyncGroups::submit` and `SyncGroups::refuse`. What a holder
/// of this type can do is [`assignment_for`], which reads.
#[derive(Clone, Debug, Default)]
pub(super) struct Assignments(Arc<Mutex<Option<Assignment>>>);

impl Assignments {
    /// ⚠️ **Private, and that is the whole mechanism.** `pub(super)` here
    /// would restore exactly the reach the newtype removes.
    fn guard(&self) -> std::sync::MutexGuard<'_, Option<Assignment>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Every group's own assignment barrier: known once the assignment-bearing
/// submission lands, `None` until then.
#[derive(Debug, Default)]
pub(crate) struct SyncGroups {
    entries: Mutex<HashMap<GroupId, Entry>>,
}

#[derive(Debug, Default)]
struct Entry {
    assignments: Assignments,
    notify: Arc<Notify>,
    /// The largest `rebalance_timeout_ms` among the members of this group's
    /// open round, each capped at
    /// [`super::deadline::MAX_MEMBER_SYNC_WAIT_MS`] — `M4.66` for why the
    /// largest, `M4.83` for why the *round's* members rather than every member
    /// that ever enrolled. It is the value a follower's own wait is derived
    /// from. `None` until a member has joined through `join_group::round`,
    /// which is every test that drives the coordinator directly and nothing a
    /// real client produces; once set it is replaced by each round rather than
    /// cleared — see [`SyncGroups::set_rebalance_timeout`].
    rebalance_timeout: Option<Duration>,
}

/// What the group's cell currently says about `generation`.
#[derive(Debug)]
pub(super) enum Known {
    /// This generation's assignment, ready to answer with.
    Mine(Arc<HashMap<String, Vec<u8>>>),
    /// The cell holds a *later* generation: the group rebalanced again while
    /// this member was waiting, and its own generation is never coming.
    Superseded,
    /// This generation's leader was refused: its assignment is not coming,
    /// and unlike [`Known::Waiting`] that is already known. Answered the
    /// same way as [`Known::Superseded`], because a client rejoins on
    /// either.
    Refused,
    /// Nothing yet, or an older generation still. Keep waiting.
    Waiting,
}

/// What the group's cell currently says about `generation`.
pub(super) fn assignment_for(assignments: &Assignments, generation: i32) -> Known {
    let guard = assignments.guard();
    // ⚠️ **An `Ordering` match, not comparison guards.** The three cases are
    // exhaustive and mutually exclusive, and spelling them that way says so
    // to the compiler instead of relying on arm order — with guards, an
    // earlier `==` arm makes the `>` in a later one unmutatable-but-live
    // (`cargo mutants` reports `>` -> `>=` surviving, because equality never
    // reaches it), which is a branch no test can pin. Found by the gate.
    let known = guard.as_ref().map_or(Known::Waiting, |a| {
        match a.generation.cmp(&generation) {
            std::cmp::Ordering::Equal => a
                .map
                .as_ref()
                .map_or(Known::Refused, |map| Known::Mine(Arc::clone(map))),
            // ⚠️ **A later generation means this waiter's own is never
            // coming**, and saying so at once is the difference between a
            // retry and a dead consumer. Without this the member sleeps out
            // the whole `MAX_SYNC_WAIT_MS` — 50 minutes — and is then
            // answered `UNKNOWN_SERVER_ERROR`, which the Java consumer
            // raises out of `poll()` and never retries; real Kafka releases
            // exactly this case immediately with `REBALANCE_IN_PROGRESS`,
            // which is what a client rejoins on. The same lesson `M4.15d`
            // learned four times over: a condition that is nobody's fault
            // must not reach a fatal code.
            // Found by review.
            std::cmp::Ordering::Greater => Known::Superseded,
            // An older generation still on the cell: this member's own
            // leader has not submitted yet. Keep waiting.
            std::cmp::Ordering::Less => Known::Waiting,
        }
    });
    drop(guard);
    known
}

impl SyncGroups {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<GroupId, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `group`'s own current entry, creating one if this is its first
    /// `SyncGroup` for the generation now completing.
    pub(super) fn entry_for(&self, group: &GroupId) -> (Assignments, Arc<Notify>) {
        let mut entries = self.lock();
        let entry = entries.entry(group.clone()).or_default();
        let handles = (entry.assignments.clone(), Arc::clone(&entry.notify));
        drop(entries);
        handles
    }

    /// Records what `rebalance_timeout_ms` `group`'s current round asks for,
    /// so a follower parking on the barrier can be bounded by the same value
    /// the join barrier was — `M4.66`, and `sync_group::deadline`'s own doc
    /// for why nothing else bounds this phase.
    ///
    /// ⚠️ **This function is last-writer-wins, and the *caller* is what makes
    /// that safe.** `from_round` is already the maximum over a roster, folded
    /// by `join_group::round` under the lock that owns it, and the two writers
    /// — a join and a withdrawal — hold that lock across this call so they
    /// cannot interleave. Review measured the version that read there and
    /// wrote here: a third member's larger fold could win the race and re-pin
    /// the group with a departed member's number. ⚠️ **The fold is a maximum,
    /// not the newest member's ask**, which the first version of `M4.66` got
    /// wrong: both reference clients seed `rebalance_timeout_ms` from
    /// `max.poll.interval.ms`, which an application may legitimately set to a
    /// second, and letting whichever member happened to open the round set the
    /// group's number truncated the phase for everyone — refusing followers
    /// before a leader's `SyncComplete` append could land, and re-opening the
    /// round they were refused into. Real Kafka folds `max` over the members'
    /// `rebalanceTimeoutMs` for exactly this.
    ///
    /// ⚠️ **A member that was refused cannot contribute, and no guard here
    /// says so** — `M4.83` deleted the one that did. The fold is over
    /// `round.members`, and a refused member was never pushed into it; stating
    /// the same fact twice, once as a condition on the caller, is what went
    /// stale in `M4.74`. `join_group::round`'s own `join` carries the reason.
    ///
    /// ⚠️ **Derived from the open round's roster, not accumulated — `M4.83`,
    /// and until then this was monotonic over a group's whole life.** The old
    /// `note_rebalance_timeout` folded `max` over every member that ever
    /// enrolled and nothing removed an `Entry`, so a member that reached
    /// `Pending` asking the ceiling and then disconnected held its group's
    /// sync backstop there until the process ended: `withdraw` took it off the
    /// roster and nothing un-noted it. Cross-principal, since `GroupGrants` is
    /// deferred. ⚠️ **`M4.76` bounded the value and not its permanence** —
    /// clamping a single member's contribution at
    /// [`super::deadline::MAX_MEMBER_SYNC_WAIT_MS`] moved the surviving route
    /// from fifty minutes to thirty and left the route itself intact. This is
    /// the repair both rows recorded as residue, and real Kafka's own shape:
    /// the fold is over the round's *current* members, so a member that leaves
    /// stops counting.
    ///
    /// `caller` passes `None` when it has nothing to say — no open round, or a
    /// withdrawal that removed nothing — and the stored value is **left
    /// alone**, never cleared. A round that has closed still has followers
    /// parked on the number it closed with, and
    /// [`super::deadline::sync_wait_ms`] answers `None` with
    /// [`super::deadline::MAX_SYNC_WAIT_MS`] — so clearing would put every one
    /// of them back on the fifty minutes `M4.66` took them off.
    pub(crate) fn set_rebalance_timeout(&self, group: &GroupId, from_round: Option<Duration>) {
        let Some(fold) = from_round else {
            return;
        };
        // ⚠️ **Still clamped per member — `M4.76`.** The value arriving here
        // is a maximum over members, so clamping it is clamping each of them:
        // `min` distributes over `max`. `super::deadline::MAX_MEMBER_SYNC_WAIT_MS`'s
        // own doc has the measurement and why the bound is the session ceiling.
        let contributed = fold.min(Duration::from_millis(
            super::deadline::MAX_MEMBER_SYNC_WAIT_MS,
        ));
        let mut entries = self.lock();
        entries.entry(group.clone()).or_default().rebalance_timeout = Some(contributed);
        drop(entries);
    }

    /// The value [`Self::set_rebalance_timeout`] last recorded for
    /// `group`.
    ///
    /// ⚠️ **A read, and it takes no entry.** The first version used
    /// `entry(..).or_default()`, which clones the `GroupId` and inserts on
    /// every follower's request; that it did no harm rested on `entry_for`
    /// having inserted one line earlier in `handle`, which is not a property
    /// of this function. Found by review.
    pub(crate) fn rebalance_timeout(&self, group: &GroupId) -> Option<Duration> {
        let entries = self.lock();
        let noted = entries.get(group).and_then(|e| e.rebalance_timeout);
        drop(entries);
        noted
    }

    /// Submits `group`'s own assignment map for `generation` — the leader's
    /// own call.
    ///
    /// ⚠️ **Replaces the previous generation's, and wakes every waiter on
    /// the group's own durable `Notify`.** Followers for this generation
    /// that arrived first are parked on it; followers still holding a
    /// *stale* generation see the new one on waking and re-check rather
    /// than being answered from a map that was never about them.
    ///
    /// `None` means the cell already holds a **later** generation and this
    /// submission is refused — the caller answers `REBALANCE_IN_PROGRESS`.
    ///
    /// ⚠️ **The guard is `>`, not `>=` or `!=`: a leader resubmitting its
    /// own generation still replaces its own map**, which is the retry path
    /// `a_resubmitted_assignment_for_the_same_generation_replaces_the_first`
    /// and `durability::a_second_submission_against_an_already_stable_group_is_still_answered`
    /// both pin — a leader retrying after a lost response would otherwise be
    /// refused and burn a generation. `>=` is the mutation that matters and
    /// turns all three red.
    ///
    /// ⚠️ **`still_at_this_generation` is asked under the cell's own lock,
    /// and `M4.65` is why it is a closure rather than a `bool`.** The guard
    /// above cannot see the case it needs to: `submit_assignment` judged
    /// `synced` from the record its `SyncComplete` transition returned and
    /// never re-read it, so a member lost between that transition landing
    /// and the leader's task resuming took the group off this generation,
    /// published `{generation, None}` for it through [`Self::refuse`] — and
    /// the leader arrived here anyway and overwrote the refusal. Followers
    /// that had just been told to rejoin read [`Known::Mine`] instead and
    /// were answered `NONE` with a slice for a generation the group had
    /// left: the revoke-before-reassign harm `M4.42` and `M4.45` exist to
    /// prevent, through the writer `M4.47` added to prevent the opposite
    /// one.
    ///
    /// ⚠️ **A re-read *before* the call would not have closed it.** The two
    /// statements are synchronous, so nothing on this runtime thread can
    /// interleave — but the transitions actor is its own task and may be
    /// running on another. Asking the question here puts it on the same side
    /// of the lock as the write it authorises, and on the same side as
    /// `refuse`'s: whichever writer takes the lock first settles the
    /// generation, which is the semantics `heartbeat.rs`'s own note already
    /// claims when it calls `refuse` on every removal.
    ///
    /// ⚠️ **The cell alone cannot answer it**, which is the reason this is
    /// not simply the mirror of `refuse`'s guard: `{generation, None}` also
    /// means *this* generation's leader was refused by a transient append
    /// failure and is retrying, with the group still on it —
    /// `durability::the_same_leader_succeeds_once_the_log_heals`. Only the
    /// coordinator's record distinguishes the two.
    ///
    /// ⚠️ **Lock order is this cell, then whatever the closure takes**, and
    /// `async-concurrency.md` rule 9 wants that stated rather than assumed.
    /// The only closure passed today reads a [`GroupCoordinator`] record, and
    /// no path in the crate takes a coordinator's lock and then this one:
    /// the coordinator is reached through the transitions actor, which holds
    /// nothing of its own across the reply, and every `refuse` caller has
    /// released what it held before calling (`heartbeat.rs` after its
    /// `await`, `join_group::round` after `drop(entries)`). ⚠️ **The
    /// signature does not enforce it**, and a closure that reached back into
    /// `SyncGroups` would deadlock on `self.lock()` here — non-reentrant, and
    /// held. The argument is narrow by construction, so keep it that way: a
    /// question about state this cell does not hold, and nothing else.
    ///
    /// ⚠️ **`M4.35`: this was an unconditional write, and it was the one
    /// blind post-`await` write M4 left.** `handle`'s fence reads the
    /// group's record synchronously and the submission then awaits a full
    /// round trip through the single transitions actor, so a generation
    /// N submission can arrive after N+1's leader has already submitted —
    /// and the stale map then replaced it, leaving every N+1 follower not
    /// yet answered looking at a cell that had gone backwards, waiting out
    /// `MAX_SYNC_WAIT_MS` for an `UNKNOWN_SERVER_ERROR`. Every other
    /// post-await write in M4 has this guard: `CommittedOffsets::apply`
    /// compares a `CommitVersion`, `is_own_open_round` pointer-compares the
    /// round, `close_on_deadline` re-reads `outcome`. Found by M4's closing
    /// review.
    pub(super) fn submit(
        &self,
        group: &GroupId,
        generation: i32,
        map: HashMap<String, Vec<u8>>,
        still_at_this_generation: impl FnOnce() -> bool,
    ) -> Option<Arc<HashMap<String, Vec<u8>>>> {
        let mut entries = self.lock();
        let entry = entries.entry(group.clone()).or_default();
        let map = Arc::new(map);
        let mut guard = entry.assignments.guard();
        let superseded = guard.as_ref().is_some_and(|a| a.generation > generation);
        if superseded || !still_at_this_generation() {
            drop(guard);
            drop(entries);
            return None;
        }
        *guard = Some(Assignment {
            generation,
            map: Some(Arc::clone(&map)),
        });
        drop(guard);
        let notify = Arc::clone(&entry.notify);
        drop(entries);
        notify.notify_waiters();
        Some(map)
    }

    /// Publishes that `generation`'s leader was refused, and wakes every
    /// follower parked on it.
    ///
    /// ⚠️ **It never overwrites an assignment, only absence.** Two ways the
    /// cell can already hold one that must survive: a *later* generation's,
    /// which is `submit`'s own `>` guard and the thing `M4.35` exists to
    /// protect; and *this* generation's, published by another connection
    /// that got there first — unpublishing that would strand every follower
    /// it was about, which is the failure this method was added to end,
    /// reached from the other side. A leader's retry after the log heals
    /// goes through `submit`, which does replace, so nothing needs `refuse`
    /// to.
    pub(crate) fn refuse(&self, group: &GroupId, generation: i32) {
        let mut entries = self.lock();
        let entry = entries.entry(group.clone()).or_default();
        let mut guard = entry.assignments.guard();
        // ⚠️ **`map.is_some()` has to be qualified by the generation, and
        // the first version of this was not** — so an *older* generation's
        // map, which every group that has ever synced leaves behind,
        // suppressed the refusal and the fix did not fire at all for the
        // common case. Overwriting an older map is safe: a follower still
        // on that generation then reads `Known::Superseded`, which is
        // already the answer it must get. Found by review.
        let holds_an_assignment = guard.as_ref().is_some_and(|a| {
            a.generation > generation || (a.generation == generation && a.map.is_some())
        });
        if holds_an_assignment {
            drop(guard);
            drop(entries);
            return;
        }
        *guard = Some(Assignment {
            generation,
            map: None,
        });
        drop(guard);
        let notify = Arc::clone(&entry.notify);
        drop(entries);
        notify.notify_waiters();
    }
}
