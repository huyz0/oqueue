//! `Cluster`'s own replay-readiness accessors — split out of `cluster.rs`
//! at `code-structure.md`'s 500-line boundary, `M4.15a`'s own addition
//! pushing it over. A submodule rather than a second file with its own
//! type: `Cluster`'s private fields (`replay_gate`, and `replay_in_progress`
//! reads `committed_offsets`' own readiness indirectly through it) are
//! visible here because a submodule inherits its parent's own visibility,
//! the same reason `cluster::tests` already can.

use super::Cluster;

/// How many times [`Cluster::wait_until_replayed`] yields before giving up
/// and panicking — `M4.15a`'s own bound against an unbounded poll loop.
/// ⚠️ Far larger than any legitimate replay over an in-memory fake needs
/// (a handful of yields in practice) so a genuine, if very slow, replay is
/// never mistaken for a stuck one; lowering it risks exactly that false
/// failure, raising it only delays how quickly a truly stuck gate is
/// noticed.
const MAX_REPLAY_WAIT_YIELDS: u32 = 10_000;

impl Cluster {
    /// Whether a group-protocol request arriving right now should be
    /// refused `COORDINATOR_LOAD_IN_PROGRESS` — `M4.15a`.
    ///
    /// ⚠️ **Narrower than the name of the field it feeds might suggest.**
    /// This covers `M4.14`'s offset replay only; group *membership* state
    /// has no durable log of its own yet (`M4.15b`), so nothing this flag
    /// gates today can actually be stale in that sense — it exists so the
    /// window is closed the moment `M4.15b` gives it something real to
    /// close, rather than needing every one of the five call sites
    /// touched again then.
    pub(crate) fn replay_in_progress(&self) -> bool {
        self.replay_gate.still_loading()
    }

    /// Waits until this cluster's own background replay (`M4.15a`) has
    /// finished — for tests and tools that want the pre-`M4.15a` "not
    /// ready until fully replayed" semantics `Cluster::new` itself no
    /// longer provides directly.
    ///
    /// ⚠️ **A real server does not call this.** `bin/oqueue`'s own
    /// `serve.rs` starts accepting connections the instant `Cluster::new`
    /// returns, on purpose — that is the whole point of moving replay to
    /// the background: `crate::fencing`'s own gate answers
    /// `COORDINATOR_LOAD_IN_PROGRESS` for the window this method exists to
    /// let a *test* skip past instead.
    ///
    /// # Panics
    ///
    /// If the background replay task ends (returns or panics) without ever
    /// marking itself ready — `async-concurrency.md` rule 13: `Cluster`
    /// owns the task's own `JoinHandle` for exactly this, so a panic
    /// inside `CommittedOffsets::replay` surfaces here (via the handle's
    /// own `JoinError`) instead of being silently swallowed by a dropped
    /// handle. Or if replay has not finished after
    /// [`MAX_REPLAY_WAIT_YIELDS`] — `security.md` rule 13's own instinct
    /// against an unbounded hold, applied to a poll loop rather than a
    /// retry: an unbounded `while ... { yield_now().await }` would spin
    /// forever rather than fail loudly the one time replay genuinely never
    /// completes, which nothing in this suite's own timeout-free
    /// `cargo test` invocation (`bin/oqueue/src/serve.rs`'s own test
    /// module names this gap) would ever surface as anything but a hung
    /// CI run.
    pub async fn wait_until_replayed(&self) {
        for _ in 0..MAX_REPLAY_WAIT_YIELDS {
            if !self.replay_in_progress() {
                return;
            }
            let finished_task = {
                let mut guard = self
                    .replay_task
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if guard
                    .as_ref()
                    .is_some_and(tokio::task::JoinHandle::is_finished)
                {
                    guard.take()
                } else {
                    None
                }
            };
            if let Some(handle) = finished_task {
                // The task ended without calling `mark_ready` — a returned
                // `Err` (already the expected, documented degradation) or a
                // panic. Awaiting the handle here, outside the lock,
                // surfaces which: a panic's own message comes back through
                // the `JoinError`, which is more than the generic timeout
                // message below could ever say.
                if let Err(join_error) = handle.await {
                    panic!("Cluster's own background replay task panicked: {join_error}");
                }
                panic!(
                    "Cluster's own background replay task ended without completing \
                     replay (CommittedOffsets::replay returned Err)"
                );
            }
            tokio::task::yield_now().await;
        }
        panic!(
            "Cluster::wait_until_replayed gave up after {MAX_REPLAY_WAIT_YIELDS} yields -- \
             replay never finished"
        );
    }
}
