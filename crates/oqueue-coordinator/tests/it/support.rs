//! Fixtures every suite in this binary shares.
//!
//! ⚠️ One copy, not three: the `RefusableLog` below is a fake of a seam, and
//! `contracts.md` rule 10 makes a fake's job fidelity to the *documented*
//! contract — three copies of it would be three meanings of guarantee 3.
//! `M3.18` moves it beside the trait in `oqueue-core`, which is where rule 9
//! says a fake belongs; until it does, this is one place rather than three.

#![allow(clippy::expect_used)]
// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module; `pub(crate)` is the visibility that is actually
// true of these fixtures, so the lint that disagrees is the one allowed. Same
// trade, same reason, as `allocator.rs`.
#![allow(clippy::redundant_pub_crate)]

use oqueue_coordinator::Coordinator;
use oqueue_core::{
    BoxFuture, ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error,
    FakeMaterializedIndex, FakeMetadataLog, MaterializedIndex, MetadataEntry, MetadataLog,
    ObjectKey, Offset, PartitionId, Result, TopicId,
};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(crate) fn topic() -> TopicId {
    TopicId::new("t".to_owned()).expect("literal is a valid topic id")
}

pub(crate) fn partition() -> PartitionId {
    PartitionId::new(0).expect("0 is a valid partition")
}

pub(crate) fn object(n: usize) -> ObjectKey {
    ObjectKey::new(format!("o-{n}")).expect("literal is a valid object key")
}

pub(crate) fn span(records: u32) -> CommittedSpan {
    CommittedSpan::new(topic(), partition(), records, ByteRange::Full)
}

/// A span of a *bundled* object: a real region rather than the whole of it.
///
/// ⚠️ [`ByteRange::Full`] is only correct for an object holding one span, so
/// the multi-span tests below name bounded regions — two spans both claiming
/// the whole object are not two regions.
pub(crate) fn region(name: &str, partition: i32, records: u32, at: u64) -> CommittedSpan {
    CommittedSpan::new(
        TopicId::new(name.to_owned()).expect("literal is a valid topic id"),
        PartitionId::new(partition).expect("a valid partition"),
        records,
        ByteRange::bounded(at, 16).expect("a non-empty range"),
    )
}

/// A [`MetadataLog`] whose `append` can be made to refuse, so a test can watch
/// what the coordinator does when the journal step fails.
///
/// ⚠️ A fake rather than a mock (`testing.md` rule 3): it delegates to
/// [`FakeMetadataLog`] and adds one switch, so what it stores when it is *not*
/// refusing is the real contract rather than a recorded expectation.
#[derive(Debug)]
pub(crate) struct RefusableLog {
    inner: FakeMetadataLog,
    refusing: AtomicBool,
    refusing_reads: AtomicBool,
}

impl RefusableLog {
    pub(crate) const fn new() -> Self {
        Self {
            inner: FakeMetadataLog::new(),
            refusing: AtomicBool::new(false),
            refusing_reads: AtomicBool::new(false),
        }
    }

    pub(crate) fn refuse(&self, refusing: bool) {
        self.refusing.store(refusing, Ordering::SeqCst);
    }

    /// Makes every read fail, so a test can prove a path never reads.
    pub(crate) fn refuse_reads(&self, refusing: bool) {
        self.refusing_reads.store(refusing, Ordering::SeqCst);
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.len()
    }
}

impl MetadataLog for RefusableLog {
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.refusing.load(Ordering::SeqCst) {
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
        if self.refusing_reads.load(Ordering::SeqCst) {
            return Box::pin(async { Err(Error::Transient) });
        }
        self.inner.read_from(start, max_entries)
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        self.inner.last_version()
    }
}

/// Opens a coordinator over `log` and spawns its serializing loop, returning
/// the handle and the loop's join handle.
pub(crate) async fn start(log: Arc<dyn MetadataLog>) -> (Coordinator, tokio::task::JoinHandle<()>) {
    start_with(log, Arc::new(FakeMaterializedIndex::new())).await
}

/// Opens over a caller-supplied index, for the tests that look at it.
pub(crate) async fn start_with(
    log: Arc<dyn MetadataLog>,
    index: Arc<dyn MaterializedIndex>,
) -> (Coordinator, tokio::task::JoinHandle<()>) {
    let (coordinator, driver) = Coordinator::open(log, index, CoordinatorEpoch::ZERO)
        .await
        .expect("a fresh log opens");
    (coordinator, tokio::spawn(driver.run()))
}

pub(crate) fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

/// A park that fails instead of hanging.
///
/// ⚠️ **Not a sleep, and not an assertion about duration** (`testing.md` rules
/// 7 and 11). Every test using it runs under `start_paused`, so the clock only
/// moves when nothing else can — the timer fires the instant the runtime goes
/// idle with the park unresolved, in no wall-clock time at all. What it buys
/// is that a wakeup that never comes is a **failure** rather than a stalled
/// run, which is the difference `oqueue-core`'s `test_executor` was written to
/// make, for the same reason: mutation testing turns a hanging test from a
/// nuisance into a stalled suite that reports nothing.
pub(crate) async fn parked<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .expect("the park was woken")
}

/// A coordinator over a fresh log, with its index returned so a test can look
/// at what a parked reader would query.
pub(crate) async fn start_indexed() -> (
    Coordinator,
    Arc<dyn MaterializedIndex>,
    tokio::task::JoinHandle<()>,
) {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let index: Arc<dyn MaterializedIndex> = Arc::new(FakeMaterializedIndex::new());
    let (coordinator, driver) = Coordinator::open(log, Arc::clone(&index), CoordinatorEpoch::ZERO)
        .await
        .expect("a fresh log opens");
    (coordinator, index, tokio::spawn(driver.run()))
}
