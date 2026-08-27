//! A [`MetadataLog`] decorator that can be made to fail, and to *stall*.
//!
//! ⚠️ **Beside the trait, because a fake belongs where the contract does**
//! (`contracts.md` rule 9). `oqueue-coordinator`'s suite grew its own "log
//! that can refuse" and carried a note saying it belonged here; rule 10 is why
//! that note mattered, since a fake's job is fidelity to the *documented*
//! contract and a copy per crate is a meaning per crate. This is the one.
//!
//! ⚠️ **A decorator over any [`MetadataLog`], not a second fake.** What it adds
//! is refusal and delay; what it stores when it is doing neither is whatever
//! the log underneath stores, so a test that stops injecting is testing the
//! real contract rather than a recorded expectation. Same shape as
//! [`CountingObjectStore`](crate::CountingObjectStore) beside `ObjectStore`.
//!
//! ⚠️ **The stall is the capability the refusal switch could not give.** A
//! `Result` says what happened after the fact; `ADR-0024`'s serialization
//! property is about what happens *during* — whether a `drop_cache` can land
//! between a commit's journal append and its fold. Nothing can race that
//! without a way to hold the append open, so the property was argued rather
//! than asserted for three tasks.

use crate::{BoxFuture, CommitVersion, Error, MetadataEntry, MetadataLog, Result};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

/// What a [`FaultMetadataLog`] should do instead of its job.
///
/// ⚠️ **Every field is off by default**, so a test names only the fault it is
/// about and a new field cannot silently change what an existing test means.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogFaults {
    /// Every `append` resolves [`Error::Transient`] without storing anything.
    pub refuse_append: bool,
    /// Every read resolves [`Error::Transient`], so a test can prove a path
    /// never reads.
    ///
    /// ⚠️ **`last_version` too, not only `read_from`** (`M3.34`). It was
    /// `read_from` alone, and the sentence above was false of the one caller
    /// that matters most: `Coordinator::open` reads a log's *last version* and
    /// nothing else, so a test injecting a read failure at startup got a
    /// coordinator that opened cleanly. A fault switch that misses a method is
    /// worse than none, because a test written against it passes.
    pub refuse_read: bool,
    /// Every `append` parks before doing anything, and stays parked until
    /// [`FaultMetadataLog::release`] is called.
    ///
    /// ⚠️ **Parks *before* storing**, which is the only position that makes
    /// the race worth running: the caller is inside the journal step with
    /// nothing durable yet, which is exactly the window `ADR-0020` makes the
    /// serialization point and `ADR-0024` says nothing else may enter.
    ///
    /// ⚠️ **Not under a paused clock.** The park wakes itself before every
    /// `Pending`, so a runtime started with `start_paused` always has a ready
    /// task and its auto-advance — which fires only when nothing is runnable —
    /// never does. A test that holds an append and then waits for a deadline
    /// hangs rather than failing. Hold appends under a real clock, or do not
    /// wait on time while one is held.
    pub hold_append: bool,
}

/// A [`MetadataLog`] that can be told to refuse or to stall.
#[derive(Debug)]
pub struct FaultMetadataLog<L> {
    inner: L,
    refuse_append: AtomicBool,
    refuse_read: AtomicBool,
    hold_append: AtomicBool,
    parked: AtomicUsize,
}

impl<L> FaultMetadataLog<L> {
    /// Wraps `inner`, injecting nothing until told to.
    pub const fn new(inner: L) -> Self {
        Self {
            inner,
            refuse_append: AtomicBool::new(false),
            refuse_read: AtomicBool::new(false),
            hold_append: AtomicBool::new(false),
            parked: AtomicUsize::new(0),
        }
    }

    /// Replaces what this log is injecting.
    pub fn set_faults(&self, faults: LogFaults) {
        self.refuse_append
            .store(faults.refuse_append, Ordering::SeqCst);
        self.refuse_read.store(faults.refuse_read, Ordering::SeqCst);
        self.hold_append.store(faults.hold_append, Ordering::SeqCst);
    }

    /// Lets every parked `append` proceed, and stops parking new ones.
    pub fn release(&self) {
        self.hold_append.store(false, Ordering::SeqCst);
    }

    /// How many `append` calls are parked right now.
    ///
    /// ⚠️ **What a racing test waits on.** "Spawn the commit, then drop the
    /// cache" is a race the spawn might lose; "spawn the commit, wait until it
    /// is *inside* the append, then drop the cache" is not a race at all.
    #[must_use]
    pub fn parked(&self) -> usize {
        self.parked.load(Ordering::SeqCst)
    }

    /// The log underneath, for the assertions that are about what it stored.
    pub const fn inner(&self) -> &L {
        &self.inner
    }
}

impl<L: MetadataLog> MetadataLog for FaultMetadataLog<L> {
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            Gate::new(self).await;
            if self.refuse_append.load(Ordering::SeqCst) {
                return Err(Error::Transient);
            }
            self.inner.append(entries).await
        })
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<MetadataEntry>>> {
        // ⚠️ **Sampled inside the future, not when the method is called**, the
        // same as `append` above and for the reason `MetadataLog`'s own fake
        // states: every method does its work inside the future it returns. A
        // decorator whose two halves disagreed about *when* a fault takes
        // effect would answer `Ok` to a read a test had already asked to fail.
        Box::pin(async move {
            if self.refuse_read.load(Ordering::SeqCst) {
                return Err(Error::Transient);
            }
            self.inner.read_from(start, max_entries).await
        })
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        Box::pin(async move {
            if self.refuse_read.load(Ordering::SeqCst) {
                return Err(Error::Transient);
            }
            self.inner.last_version().await
        })
    }
}

/// Resolves once the log is not holding appends.
///
/// ⚠️ **Self-waking, like [`FaultConfig`](crate::FaultConfig)'s delay**, and
/// for the same reason: nothing in this crate compiles a clock or a channel
/// (`AGENTS.md` non-negotiable 5), so a park that waited for a wakeup nobody
/// sends would hang a real executor rather than stall one call.
struct Gate<'a, L> {
    log: &'a FaultMetadataLog<L>,
    counted: bool,
}

impl<'a, L> Gate<'a, L> {
    const fn new(log: &'a FaultMetadataLog<L>) -> Self {
        Self {
            log,
            counted: false,
        }
    }
}

impl<L> Future for Gate<'_, L> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if !self.log.hold_append.load(Ordering::SeqCst) {
            if self.counted {
                self.log.parked.fetch_sub(1, Ordering::SeqCst);
                self.counted = false;
            }
            return Poll::Ready(());
        }
        if !self.counted {
            self.log.parked.fetch_add(1, Ordering::SeqCst);
            self.counted = true;
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<L> Drop for Gate<'_, L> {
    fn drop(&mut self) {
        // ⚠️ A cancelled `append` was parked and now is not — a count that
        // only ever went up would make `parked()` a high-water mark, and the
        // tests that wait on it would pass on a call that had been dropped.
        if self.counted {
            self.log.parked.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{FaultMetadataLog, LogFaults};

    use crate::{
        CommitVersion, CommittedSpan, Error, FakeMetadataLog, MetadataEntry, MetadataLog,
        MetadataRecord, ObjectKey, PartitionId, TopicId,
    };
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    fn entry(version: u64) -> MetadataEntry {
        let topic = TopicId::new("t".to_owned()).expect("a valid topic");
        let partition = PartitionId::new(0).expect("a valid partition");
        MetadataEntry::new(
            CommitVersion::new(version),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new("o".to_owned()).expect("a valid key"),
                spans: vec![CommittedSpan::new(
                    topic,
                    partition,
                    1,
                    crate::ByteRange::Full,
                )],
            },
        )
    }

    fn log() -> FaultMetadataLog<FakeMetadataLog> {
        FaultMetadataLog::new(FakeMetadataLog::new())
    }

    /// ⚠️ **Injecting nothing is the default**, so a test names only the fault
    /// it is about and what the decorator stores otherwise is whatever the log
    /// underneath stores — `contracts.md` rule 10's whole point.
    #[test]
    fn a_decorator_injecting_nothing_is_the_log_it_wraps() {
        let log = log();
        let entries = [entry(0)];
        drive(log.append(&entries)).expect("nothing is being injected");
        assert_eq!(log.inner().len(), 1);
        assert_eq!(
            drive(log.read_from(CommitVersion::ZERO, 10)).expect("nor here"),
            vec![entry(0)]
        );
        assert_eq!(
            drive(log.last_version()).expect("nor here"),
            Some(CommitVersion::ZERO)
        );
    }

    /// ⚠️ **A refused append stores nothing**, which is the difference between
    /// a fault and a lie: a decorator that refused *after* delegating would
    /// leave the log holding an entry the caller was told did not land.
    #[test]
    fn a_refused_append_is_refused_before_the_log_sees_it() {
        let log = log();
        log.set_faults(LogFaults {
            refuse_append: true,
            ..LogFaults::default()
        });
        let entries = [entry(0)];
        assert!(matches!(drive(log.append(&entries)), Err(Error::Transient)));
        assert_eq!(log.inner().len(), 0, "the refusal came first");

        log.set_faults(LogFaults::default());
        drive(log.append(&entries)).expect("and it stops when told to");
        assert_eq!(log.inner().len(), 1);
    }

    /// ⚠️ **Reads refuse separately from appends**, so a test can prove a path
    /// never reads without also making it unable to write.
    ///
    /// ⚠️ **Both reads**, which `M3.34` found the switch was not covering:
    /// `Coordinator::open` reads a log's `last_version` and nothing else, so a
    /// startup test injecting a read failure got a coordinator that opened
    /// cleanly and an assertion that passed for the wrong reason.
    #[test]
    fn a_refused_read_refuses_every_read_and_leaves_appends_alone() {
        let log = log();
        log.set_faults(LogFaults {
            refuse_read: true,
            ..LogFaults::default()
        });
        let entries = [entry(0)];
        drive(log.append(&entries)).expect("appends are unaffected");
        assert!(matches!(
            drive(log.read_from(CommitVersion::ZERO, 10)),
            Err(Error::Transient)
        ));
        assert!(
            matches!(drive(log.last_version()), Err(Error::Transient)),
            "a log's last version is a read of it too"
        );
    }

    /// ⚠️ **A held append parks, and stays parked**, which is the capability
    /// the refusal switch could not give: a `Result` says what happened after
    /// the fact, and `ADR-0024`'s property is about what may happen *during*.
    #[test]
    fn a_held_append_parks_until_it_is_released() {
        let log = log();
        log.set_faults(LogFaults {
            hold_append: true,
            ..LogFaults::default()
        });
        assert_eq!(log.parked(), 0, "nothing is in there yet");

        let entries = [entry(0)];
        let mut appending = Box::pin(log.append(&entries));
        assert!(poll_once(&mut appending).is_pending(), "it parked");
        assert_eq!(log.parked(), 1, "and says so");
        assert_eq!(log.inner().len(), 0, "with nothing stored yet");

        log.release();
        drive_pinned(&mut appending).expect("it resolves once released");
        assert_eq!(log.parked(), 0);
        assert_eq!(log.inner().len(), 1, "and then it stores");
    }

    /// ⚠️ **A cancelled append was parked and now is not.** A count that only
    /// went up would make `parked` a high-water mark, and a test waiting on it
    /// would proceed against a call that had already been dropped — passing
    /// for a reason it did not mean.
    #[test]
    fn a_dropped_append_stops_being_parked() {
        let log = log();
        log.set_faults(LogFaults {
            hold_append: true,
            ..LogFaults::default()
        });
        let entries = [entry(0)];
        let mut appending = Box::pin(log.append(&entries));
        assert!(poll_once(&mut appending).is_pending());
        assert_eq!(log.parked(), 1);

        drop(appending);
        assert_eq!(log.parked(), 0, "the window closed when the caller left");
    }

    /// Polls `future` exactly once, with a waker that does nothing.
    fn poll_once<F: Future>(future: &mut Pin<Box<F>>) -> Poll<F::Output> {
        let waker = std::task::Waker::noop();
        future.as_mut().poll(&mut Context::from_waker(waker))
    }

    /// Polls until ready, or panics after a bound.
    ///
    /// ⚠️ **Bounded, and that is the whole helper.** [`Gate`] wakes itself
    /// while it is closed, so a spin driver polls a stuck gate forever — and a
    /// test that hangs is a mutation *not caught*, which the mutation gate
    /// reports as a timeout rather than a kill. A ceiling turns the same
    /// defect into a failed assertion in microseconds.
    fn drive_pinned<F: Future>(future: &mut Pin<Box<F>>) -> F::Output {
        for _ in 0..1_000 {
            if let Poll::Ready(out) = poll_once(future) {
                return out;
            }
        }
        panic!("a future that should have resolved is still parked");
    }

    /// [`drive_pinned`], for a future the caller has not pinned.
    fn drive<F: Future>(future: F) -> F::Output {
        drive_pinned(&mut Box::pin(future))
    }
}
