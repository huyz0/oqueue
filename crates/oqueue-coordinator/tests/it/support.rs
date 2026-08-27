//! Fixtures every suite in this binary shares.
//!
//! ⚠️ **No fake of a seam lives here any more.** `RefusableLog` did, with a
//! note saying it belonged beside the trait in `oqueue-core` — `contracts.md`
//! rule 9 — and `M3.32` moved it: it is `FaultMetadataLog` now, a decorator
//! over any `MetadataLog` rather than a second fake of one, and it can hold an
//! append open as well as refuse it.

#![allow(clippy::expect_used)]
// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module; `pub(crate)` is the visibility that is actually
// true of these fixtures, so the lint that disagrees is the one allowed. Same
// trade, same reason, as `allocator.rs`.
#![allow(clippy::redundant_pub_crate)]

use oqueue_coordinator::Coordinator;
use oqueue_core::{
    ByteRange, CommittedSpan, CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog,
    IndexReader, MaterializedIndex, MetadataLog, ObjectKey, Offset, PartitionId, TopicId,
};
use std::future::Future;
use std::sync::Arc;
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

/// Opens a coordinator over `log` and spawns its serializing loop, returning
/// the handle and the loop's join handle.
pub(crate) async fn start(log: Arc<dyn MetadataLog>) -> (Coordinator, tokio::task::JoinHandle<()>) {
    start_with(log, Box::new(FakeMaterializedIndex::new())).await
}

/// Opens over a caller-supplied index and discards the reader.
///
/// ⚠️ A test that needs to *look* at the index calls `start_indexed`, or
/// `Coordinator::open` directly — `ADR-0024` means the index is reachable only
/// through the `IndexReader` this helper drops.
pub(crate) async fn start_with(
    log: Arc<dyn MetadataLog>,
    index: Box<dyn MaterializedIndex>,
) -> (Coordinator, tokio::task::JoinHandle<()>) {
    let (coordinator, driver, _index) = Coordinator::open(log, index, CoordinatorEpoch::ZERO)
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
pub(crate) async fn start_indexed() -> (Coordinator, IndexReader, tokio::task::JoinHandle<()>) {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver, index) = Coordinator::open(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect("a fresh log opens");
    (coordinator, index, tokio::spawn(driver.run()))
}
