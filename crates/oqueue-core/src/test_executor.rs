//! A cooperative, single-threaded executor for tests — enough to poll a set
//! of futures to completion, and no more.
//!
//! ⚠️ **Test-only, and `oqueue-core` still takes no async-runtime
//! dependency**: this whole module is `#[cfg(test)]`, so it is not in the
//! library any dependent builds. It exists because [`ObjectStore`] is an
//! async seam and some of its decorators — [`crate::ChunkedObjectStore`]
//! above all — coordinate *between* callers, which cannot be exercised by
//! driving one future at a time.
//!
//! ⚠️ **Not a duplicate of `merge.rs`'s own spin `block_on`.** That one polls
//! a single future in a busy loop, which is correct there: nothing in
//! `merge.rs` ever parks, so a `Pending` is always transient and a spin
//! always terminates. This is a different tool for a different situation —
//! futures here genuinely park, waiting on *another* future's wake, and the
//! difference is what [`run_all`]'s deadlock detection is for.
//!
//! ⚠️ **Single-threaded, and the deadlock detection assumes it.** "Nothing is
//! runnable, so nothing ever will be" is only sound while every possible wake
//! comes from a future this executor is itself driving. Hand it a future
//! waiting on a wake from another *thread* and a legitimate wait becomes a
//! spurious "deadlock" panic. That is a safe assumption for this crate —
//! every store under test is a synchronous fake — but it is an assumption,
//! and it is why this is test support rather than something exported.
//!
//! [`ObjectStore`]: crate::ObjectStore

// This whole module is `#[cfg(test)]` — test support rather than library
// code, so the workspace's `expect_used` denial is allowed here for exactly
// the reason every `mod tests` block already allows it: a panic in a test
// harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]
// ⚠️ `redundant_pub_crate` and `unreachable_pub` contradict each other on an
// item in a private module: the first wants `pub`, the second wants
// `pub(crate)` back. `pub(crate)` is the visibility that is actually true, so
// it stays and the lint that disagrees is the one allowed.
#![allow(clippy::redundant_pub_crate)]

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Wake, Waker};

/// Re-queues its own future when woken. One per future, holding that
/// future's index into the executor's ready queue.
struct TaskWaker {
    index: usize,
    ready: Arc<Mutex<Vec<usize>>>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.index);
    }
}

/// Runs every future in `futures` to completion on this one thread, polling
/// only those whose [`Waker`] has actually been signalled.
///
/// ⚠️ **It detects a deadlock rather than hanging on one.** A spinning
/// `block_on` turns "a future that will never be woken" into an infinite
/// loop. An executor with a real ready queue knows something a spin loop
/// cannot: that when nothing is runnable while futures are still pending,
/// no future can *ever* make progress. That is a bug, and this panics on it
/// in microseconds instead of spinning until something outside the test
/// gives up.
///
/// ⚠️ **Mutation testing is what forced this**, and the finding is worth more
/// than the fix: `cargo mutants` reported `LeaderGuard::publish` and
/// `get_chunk`'s `!is_leader` as **timeouts**, not as caught. The tests did
/// detect both mutants — by hanging. A suite that detects a defect by never
/// finishing is strictly worse than one that fails, and it was degrading
/// `testing.md` rule 15's primary gate into a stalled run. Both are caught
/// outright now.
///
/// # Panics
///
/// If no future is runnable while some are still pending (a deadlock), or if
/// a future somehow finishes without leaving an output.
pub(crate) fn run_all<F: Future>(futures: Vec<F>) -> Vec<F::Output> {
    let count = futures.len();
    let mut futures: Vec<_> = futures.into_iter().map(|f| Some(Box::pin(f))).collect();
    let mut outputs: Vec<Option<F::Output>> = (0..count).map(|_| None).collect();
    // Every future starts runnable; after that, only a wake re-queues one.
    let ready = Arc::new(Mutex::new((0..count).collect::<Vec<usize>>()));
    let wakers: Vec<Waker> = (0..count)
        .map(|index| {
            Waker::from(Arc::new(TaskWaker {
                index,
                ready: Arc::clone(&ready),
            }))
        })
        .collect();

    let mut pending = count;
    while pending > 0 {
        let batch = std::mem::take(&mut *ready.lock().unwrap_or_else(PoisonError::into_inner));
        assert!(
            !batch.is_empty(),
            "deadlock: {pending} future(s) still pending and none runnable — \
             no wake can ever arrive, so nothing can make progress"
        );
        for index in batch {
            let Some(future) = futures[index].as_mut() else {
                continue; // Already finished; a stale wake queued it again.
            };
            let mut cx = Context::from_waker(&wakers[index]);
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                outputs[index] = Some(value);
                futures[index] = None;
                pending -= 1;
            }
        }
    }

    outputs
        .into_iter()
        .map(|o| o.expect("every future ran to completion"))
        .collect()
}

/// [`run_all`] for the common single-future case.
///
/// # Panics
///
/// For the same reasons [`run_all`] does.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut outputs = run_all(vec![future]);
    outputs
        .pop()
        .expect("exactly one future in, so exactly one output out")
}

#[cfg(test)]
mod tests {
    use super::{block_on, run_all};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    #[test]
    fn runs_a_single_future_to_completion() {
        assert_eq!(block_on(async { 7_u32 }), 7);
    }

    #[test]
    fn runs_many_futures_and_keeps_their_order() {
        let outputs = run_all((0..5_u32).map(|i| async move { i * 2 }).collect());
        assert_eq!(outputs, vec![0, 2, 4, 6, 8]);
    }

    /// A future that parks until another future wakes it — the shape
    /// `chunk.rs`'s follower has, and the reason this executor exists.
    struct Parked {
        cell: Arc<Mutex<(bool, Option<std::task::Waker>)>>,
    }

    impl Future for Parked {
        type Output = ();

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let mut cell = self.cell.lock().expect("uncontended");
            if cell.0 {
                return Poll::Ready(());
            }
            cell.1 = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    #[test]
    fn a_parked_future_is_resumed_by_another_future_s_wake() {
        let cell = Arc::new(Mutex::new((false, None::<std::task::Waker>)));
        let parked = Parked {
            cell: Arc::clone(&cell),
        };
        let waker_cell = Arc::clone(&cell);
        let releaser = async move {
            let waker = {
                let mut cell = waker_cell.lock().expect("uncontended");
                cell.0 = true;
                cell.1.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
        };
        // The parked future is polled first, so it is genuinely waiting when
        // the releaser runs.
        let mut done = vec![false, false];
        for (i, ()) in run_all(vec![
            Box::pin(parked) as Pin<Box<dyn Future<Output = ()>>>,
            Box::pin(releaser),
        ])
        .into_iter()
        .enumerate()
        {
            done[i] = true;
        }
        assert_eq!(done, vec![true, true]);
    }

    /// ⚠️ The property this module exists for: a future nothing will ever
    /// wake is a **panic**, not a hang.
    #[test]
    #[should_panic(expected = "deadlock")]
    fn a_future_nothing_can_wake_panics_rather_than_hanging() {
        let cell = Arc::new(Mutex::new((false, None::<std::task::Waker>)));
        run_all(vec![Parked { cell }]);
    }
}
