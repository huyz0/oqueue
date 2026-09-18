//! The object lifecycle: released → waited → deleted, capped, backpressured,
//! and quarantined when a key will not go (`M5.21`).

#![allow(clippy::expect_used)]

use crate::support::{committed, key, topic};
use oqueue_compact::{
    DELETE_BATCH_KEYS, DELETION_BACKLOG_KEYS, GcTerms, Lifecycle, QUARANTINE_AFTER_REFUSALS,
};
use oqueue_core::{
    BoxFuture, ByteRange, CommitVersion, CommittedSpan, Error, FakeMaterializedIndex,
    FakeObjectStore, MaterializedIndex, MetadataEntry, MetadataRecord, MultipartWriter, ObjectKey,
    ObjectMeta, ObjectStore, Offset, PartitionId, Precondition, Result, Timestamp,
};
use std::sync::Mutex;

const DELAY: i64 = 1_000;

/// A lifecycle over no safety terms, so a test may pick any positive delay.
fn lifecycle(delay_ms: i64) -> Lifecycle {
    let none = GcTerms {
        metadata_staleness_ms: 0,
        fetch_duration_ms: 0,
        clock_skew_ms: 0,
    };
    Lifecycle::new(delay_ms, none).expect("a positive delay exceeds nothing")
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).expect("a valid time")
}

/// A store that refuses to delete some keys and records every delete it is
/// asked for, key by key.
#[derive(Debug, Default)]
struct Refusing {
    inner: FakeObjectStore,
    refuse: Vec<ObjectKey>,
    asked: Mutex<Vec<ObjectKey>>,
}

impl Refusing {
    fn asked(&self) -> Vec<ObjectKey> {
        self.asked.lock().expect("unpoisoned").clone()
    }
}

impl ObjectStore for Refusing {
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>> {
        self.inner.get(key, range)
    }

    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<Precondition>,
    ) -> BoxFuture<'a, Result<ObjectMeta>> {
        self.inner.put(key, payload, precondition)
    }

    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> BoxFuture<'a, Result<Box<dyn MultipartWriter<'a> + 'a>>> {
        self.inner.open_multipart(key)
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>> {
        self.asked
            .lock()
            .expect("unpoisoned")
            .extend(keys.iter().cloned());
        if keys.iter().any(|k| self.refuse.contains(k)) {
            return Box::pin(async { Err(Error::Transient) });
        }
        self.inner.delete(keys)
    }
}

fn index_naming(object: &str) -> FakeMaterializedIndex {
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[committed(
            1,
            key(object),
            vec![CommittedSpan::new(
                topic(),
                PartitionId::new(0).expect("a valid partition"),
                1,
                ByteRange::Full,
                None,
            )],
        )])
        .expect("one commit");
    index
}

fn trim_everything(index: &FakeMaterializedIndex) {
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(2),
            MetadataRecord::Trimmed {
                topic: topic(),
                partition: PartitionId::new(0).expect("a valid partition"),
                start: Offset::new(1).expect("a valid offset"),
            },
        )])
        .expect("a trim to the end");
}

/// ⚠️ **The delay runs from the first sweep that sees zero**, never from the
/// release: an object still named when released is timed from when a sweep
/// finds it unnamed, so a reader of its last slice is covered.
#[tokio::test]
async fn the_delay_runs_from_the_first_sweep_that_sees_zero() {
    let index = index_naming("obj");
    let store = Refusing::default();
    let mut lifecycle = lifecycle(DELAY);
    lifecycle.release(key("obj"));

    let report = lifecycle.sweep(&index, &store, at(0)).await;
    assert_eq!(report.deleted, 0, "still referenced");
    trim_everything(&index);
    assert_eq!(index.references(&key("obj")), 0);

    let report = lifecycle.sweep(&index, &store, at(5 * DELAY)).await;
    assert_eq!(
        report.deleted, 0,
        "the first sighting of zero starts the clock"
    );
    let report = lifecycle.sweep(&index, &store, at(6 * DELAY - 1)).await;
    assert_eq!(report.deleted, 0, "one millisecond short");
    let report = lifecycle.sweep(&index, &store, at(6 * DELAY)).await;
    assert_eq!(report.deleted, 1);
    assert_eq!(store.asked(), vec![key("obj")]);
    assert_eq!(lifecycle.backlog(), 0);
}

#[tokio::test]
async fn a_referenced_object_is_never_deleted() {
    let index = index_naming("pinned");
    let store = Refusing::default();
    let mut lifecycle = lifecycle(DELAY);
    lifecycle.release(key("pinned"));
    for sweep in 0..10 {
        lifecycle
            .sweep(&index, &store, at(sweep * 10 * DELAY))
            .await;
    }
    assert!(store.asked().is_empty(), "nothing asked of the store");
    assert_eq!(lifecycle.backlog(), 1, "still logically deleted");
}

/// A key refused in `QUARANTINE_AFTER_REFUSALS` consecutive sweeps is
/// quarantined and reported, and asked for no more; the keys beside it are
/// not held back by it.
#[tokio::test]
async fn a_key_refused_repeatedly_is_quarantined_not_retried_forever() {
    let index = FakeMaterializedIndex::new();
    let store = Refusing {
        refuse: vec![key("stuck")],
        ..Refusing::default()
    };
    let mut lifecycle = lifecycle(1);
    lifecycle.release(key("stuck"));
    lifecycle.release(key("fine"));
    lifecycle.sweep(&index, &store, at(0)).await;

    let mut quarantined = Vec::new();
    let mut deleted = 0;
    for sweep in 1..=i64::from(QUARANTINE_AFTER_REFUSALS) {
        let report = lifecycle.sweep(&index, &store, at(sweep)).await;
        deleted += report.deleted;
        quarantined.extend(report.quarantined);
    }
    assert_eq!(deleted, 1, "the key beside it was deleted");
    assert_eq!(quarantined, vec![key("stuck")]);
    assert_eq!(lifecycle.quarantined(), &[key("stuck")]);

    let asked = store.asked().len();
    let report = lifecycle.sweep(&index, &store, at(1_000)).await;
    assert_eq!(report.refused, 0);
    assert_eq!(
        store.asked().len(),
        asked,
        "a quarantined key is not asked again"
    );
    lifecycle.release(key("stuck"));
    assert_eq!(
        lifecycle.backlog(),
        0,
        "nor re-admitted by a replayed release"
    );
}

/// Refusals are counted over *consecutive* sweeps: a key refused fewer times
/// than the limit stays queued and is retried.
#[tokio::test]
async fn a_key_refused_fewer_times_than_the_limit_stays_queued() {
    let index = FakeMaterializedIndex::new();
    let store = Refusing {
        refuse: vec![key("stuck")],
        ..Refusing::default()
    };
    let mut lifecycle = lifecycle(1);
    lifecycle.release(key("stuck"));
    lifecycle.sweep(&index, &store, at(0)).await;
    for sweep in 1..i64::from(QUARANTINE_AFTER_REFUSALS) {
        let report = lifecycle.sweep(&index, &store, at(sweep)).await;
        assert_eq!(report.refused, 1);
        assert!(report.quarantined.is_empty());
    }
    assert!(lifecycle.quarantined().is_empty());
    assert_eq!(lifecycle.backlog(), 1, "still queued for another try");
}

/// A sweep replayed over the same state asks the store for nothing new.
#[tokio::test]
async fn a_replayed_sweep_deletes_nothing_twice() {
    let index = FakeMaterializedIndex::new();
    let store = Refusing::default();
    let mut lifecycle = lifecycle(1);
    lifecycle.release(key("once"));
    lifecycle.release(key("once"));
    lifecycle.sweep(&index, &store, at(0)).await;
    let first = lifecycle.sweep(&index, &store, at(1)).await;
    let replay = lifecycle.sweep(&index, &store, at(1)).await;
    assert_eq!(first.deleted, 1);
    assert_eq!(replay.deleted, 0);
    assert_eq!(store.asked(), vec![key("once")], "asked exactly once");
}

/// ⚠️ **Deletion slower than creation, and the backlog still bounded.** Each
/// round the creator releases more than a sweep may delete, but only while
/// the lifecycle admits it — the backpressure a cap alone does not give.
#[tokio::test]
async fn the_backlog_is_bounded_when_deletion_is_slower_than_creation() {
    let index = FakeMaterializedIndex::new();
    let store = Refusing::default();
    let mut lifecycle = lifecycle(1);
    let per_round = 3 * DELETE_BATCH_KEYS;
    let mut minted = 0_usize;
    let mut refused_rounds = 0;
    for round in 0..80_i64 {
        if lifecycle.admits() {
            for _ in 0..per_round {
                lifecycle.release(key(&format!("garbage-{minted}")));
                minted += 1;
            }
        } else {
            refused_rounds += 1;
        }
        let report = lifecycle.sweep(&index, &store, at(round)).await;
        assert!(report.deleted <= DELETE_BATCH_KEYS, "the per-sweep cap");
        assert!(
            lifecycle.backlog() < DELETION_BACKLOG_KEYS + per_round,
            "bounded at round {round}: {}",
            lifecycle.backlog()
        );
    }
    assert!(refused_rounds > 0, "the backpressure engaged");
    assert!(
        minted >= DELETION_BACKLOG_KEYS,
        "and only once the backlog was full"
    );
}

/// Admission stops exactly at the bound, not before it.
#[test]
fn admission_stops_at_the_backlog_bound() {
    let mut lifecycle = lifecycle(1);
    for n in 0..DELETION_BACKLOG_KEYS - 1 {
        lifecycle.release(key(&format!("k-{n}")));
    }
    assert!(lifecycle.admits(), "one short of the bound");
    lifecycle.release(key("last"));
    assert!(!lifecycle.admits(), "at the bound");
}
