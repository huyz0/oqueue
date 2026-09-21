//! `Cluster`'s own replay-readiness accessors — split out of `cluster.rs`
//! at `code-structure.md`'s 500-line boundary, `M4.15a`'s own addition
//! pushing it over. A submodule rather than a second file with its own
//! type: `Cluster`'s private fields (`replay_gate`, and `replay_in_progress`
//! reads `committed_offsets`' own readiness indirectly through it) are
//! visible here because a submodule inherits its parent's own visibility,
//! the same reason `cluster::tests` already can.
//!
//! ⚠️ **`M4.15c` added `group_transitions` here too, not a third file** —
//! the field is populated by the same background task this module's own
//! `wait_until_replayed` already inspects (`GroupTransitionsTask::serve`
//! is what that task falls through into once both replays succeed), so
//! the accessor belongs beside the machinery it reads, not split again
//! for a name that would otherwise need to be `replay_and_transitions.rs`.

use super::Cluster;
use oqueue_core::{GroupCoordinator, GroupMetadataLog};
use std::sync::Arc;
use std::time::Duration;

/// How many times [`Cluster::wait_until_replayed`] polls before giving up
/// and panicking — `M4.15a`'s own bound against an unbounded poll loop.
/// ⚠️ **Read together with [`REPLAY_WAIT_POLL`]: the bound is their
/// product**, ~2 s, and that product is what matters rather than either
/// number. It was `10_000` *yields*, which is not a bound in time at all —
/// see `REPLAY_WAIT_POLL`. Lowering the product risks a false failure on a
/// legitimately slow replay; raising it delays how quickly a truly stuck
/// gate is noticed **and costs `check-mutants` wall clock**, since every
/// mutant that stalls replay waits the whole product out before failing: at
/// 10 s that gate measured ~93 s against ~40 s before, and ~72 s at 2 s.
///
/// ⚠️ **That cost is not an NFR-56 failure, and an earlier version of this
/// comment said it was.** `check-budget.sh` removes every gate over
/// `COMPILING_GATE_MS` (5 s) from the total it judges, so `check-mutants` is
/// invisible to that gate at any of these durations — `M4.21`'s backlog row
/// already recorded exactly that. The budget failure seen while choosing
/// this constant was the *warm* remainder at 10113 ms against a 10000 ms
/// budget, 113 ms of ordinary erosion, and would have happened at either
/// ceiling. So 2 s is chosen to keep the suite's wall clock down, not to
/// stay inside a budget it cannot breach — and restoring a longer bound, if
/// a real false failure ever argues for one, is a wall-clock trade rather
/// than something non-negotiable 2 forbids relieving. Two seconds is still
/// three orders of magnitude more than the one or two polls a real replay
/// over an in-memory fake needs.
const MAX_REPLAY_WAIT_POLLS: u32 = 2_000;

/// How long each of those polls waits.
///
/// ⚠️ **A sleep, not a `yield_now`, and the difference is a real flake.**
/// A yield count is not a time bound on a `multi_thread` runtime: yielding
/// reschedules *this* task, and when every worker is busy — the gate runs
/// `cargo-mutants` across 12 CPUs — ten thousand of them can elapse in
/// milliseconds without the replay task ever being scheduled. That is how
/// `dispatch::tests::api_versions_round_trips_through_a_connection`, the
/// suite's only real-clock multi-thread test, failed intermittently under
/// gate load while passing 300 consecutive runs in isolation. Sleeping
/// bounds the wait in time instead — ~2 s here, the product with
/// [`MAX_REPLAY_WAIT_POLLS`], and under load each sleep is a floor rather
/// than a ceiling, so the real bound only grows.
///
/// ⚠️ **It is still correct under `start_paused = true`.** Tokio
/// auto-advances a paused clock only once every task is idle, so a runnable
/// replay task runs *before* this sleep completes — the paused-time callers
/// see no wall-clock delay, and get the scheduling guarantee a bare yield
/// never gave them. Found by `M4.17`'s gate run.
const REPLAY_WAIT_POLL: Duration = Duration::from_millis(1);

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
    /// [`MAX_REPLAY_WAIT_POLLS`] — `security.md` rule 13's own instinct
    /// against an unbounded hold, applied to a poll loop rather than a
    /// retry: an unbounded `while ... { sleep(..).await }` would spin
    /// forever rather than fail loudly the one time replay genuinely never
    /// completes, which nothing in this suite's own timeout-free
    /// `cargo test` invocation (`bin/oqueue/src/serve.rs`'s own test
    /// module names this gap) would ever surface as anything but a hung
    /// CI run.
    pub async fn wait_until_replayed(&self) {
        for _ in 0..MAX_REPLAY_WAIT_POLLS {
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
            tokio::time::sleep(REPLAY_WAIT_POLL).await;
        }
        panic!(
            "Cluster::wait_until_replayed gave up after {MAX_REPLAY_WAIT_POLLS} polls -- \
             replay never finished"
        );
    }

    /// The single-writer seam every group-state-mutating handler enqueues
    /// through — `M4.15c` for three of them, `M4.15d` for
    /// `join_group`'s own barrier bookkeeping, which until then called
    /// `group_coordinator().transition(...)` directly. ⚠️ There is no
    /// exception left: the only direct calls in this crate are the actor's
    /// own apply and replay.
    pub(crate) const fn group_transitions(&self) -> &crate::group_transitions::GroupTransitions {
        &self.group_transitions
    }

    /// Whether `Cluster`'s own background task is still running — `true`
    /// for the whole of replay *and* for as long as
    /// `crate::group_transitions::GroupTransitionsTask::serve` keeps
    /// running afterward, since `M4.15c` both phases are the same task.
    ///
    /// ⚠️ **`async-concurrency.md` rule 13's own bar, extended past
    /// replay.** `wait_until_replayed` already gives replay's own panic an
    /// owner that observes it; once `serve` takes over, nothing calls this
    /// method automatically (no health-check surface exists yet, the same
    /// no-`tracing` gap `Cluster::new`'s own doc names), so a panic inside
    /// `serve` is not proactively *surfaced* anywhere today. But it is not
    /// *silent* either: every `GroupTransitions::enqueue`/`transition`
    /// call after `serve` dies resolves `Error::Transient` at once (a
    /// dropped `oneshot::Sender` completing the receiver with an error),
    /// never a hang and never a wrong answer — this method is what lets a
    /// caller (a test today, a future health probe) confirm *why*,
    /// non-destructively: it only inspects `is_finished`, never consumes
    /// the handle the way `wait_until_replayed` does to read a panic's own
    /// message.
    // ⚠️ No production caller exists yet — no health-check surface is
    // built (`Cluster::new`'s own doc names the same no-`tracing` gap).
    // `group_state.rs`'s own "seam now, caller later" precedent
    // (`oqueue_core::authorize`, `TopicGrants::grant`/`revoke`), applied
    // here to a diagnostic rather than a protocol seam: this row's own
    // test is the caller until a real probe exists to be the other one.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn group_transitions_task_alive(&self) -> bool {
        self.replay_task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
    }
}

/// Replays committed offsets and group transitions from the group log, opens
/// the replay gate once both have landed, then serves group transitions.
pub(super) async fn replay_groups(
    committed_offsets: Arc<crate::offset_commit::CommittedOffsets>,
    replay_gate: Arc<crate::replay_gate::ReplayGate>,
    group_transitions_task: crate::group_transitions::GroupTransitionsTask,
    group_coordinator: Arc<dyn GroupCoordinator>,
    group_metadata_log: Arc<dyn GroupMetadataLog>,
) {
    // ⚠️ **Retried until both land, each at most once** (`M6.10`):
    // a group log whose store is unreachable at boot fails its
    // first read, and giving up there left the group APIs loading
    // forever. The halves are retried separately because one that
    // succeeded must not be folded twice — replayed transitions
    // are events, not idempotent writes. A lazily opened log fails
    // at its open, before anything is folded; once open, its reads
    // are from memory.
    // ⚠️ **Readiness stays all-or-nothing**: the gate opens only
    // once both halves have replayed, so no answer trusts half.
    // ⚠️ **Only what can clear is retried**: an error its own
    // `RetryClass` calls `Never` — a log that will refuse the same
    // way every time — ends the task with the gate shut, as it did
    // before `M6.10`.
    let permanent = |result: &oqueue_core::Result<()>| matches!(result, Err(error) if error.retry_class() == oqueue_core::RetryClass::Never);
    let (mut offsets_done, mut transitions_done) = (false, false);
    loop {
        if !offsets_done {
            let replayed = committed_offsets.replay().await;
            if permanent(&replayed) {
                replay_failure("replay_offsets");
                return;
            }
            if replayed.is_err() {
                replay_failure("retry_replay_offsets");
            }
            offsets_done = replayed.is_ok();
        }
        if !transitions_done {
            let replayed = group_transitions_task
                .replay(group_coordinator.as_ref(), group_metadata_log.as_ref())
                .await;
            if permanent(&replayed) {
                replay_failure("replay_transitions");
                return;
            }
            if replayed.is_err() {
                replay_failure("retry_replay_transitions");
            }
            transitions_done = replayed.is_ok();
        }
        if offsets_done && transitions_done {
            break;
        }
        tokio::time::sleep(crate::DEGRADED_RETRY).await;
    }
    replay_gate.mark_ready();
    group_transitions_task
        .serve(group_coordinator, group_metadata_log)
        .await;
}

fn replay_failure(operation: &'static str) {
    crate::telemetry::dependency_failure("group_log", operation, 0, None, "coordinator");
}

#[cfg(test)]
mod tests {
    use super::replay_failure;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tracing::{Event, Subscriber};
    use tracing_subscriber::{
        layer::{Context, Layer},
        prelude::*,
    };

    struct Count(Arc<AtomicUsize>);

    impl<S: Subscriber> Layer<S> for Count {
        fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, S>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn replay_failures_emit_dependency_events() {
        let count = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry().with(Count(Arc::clone(&count)));
        tracing::subscriber::with_default(subscriber, || replay_failure("replay_offsets"));
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
