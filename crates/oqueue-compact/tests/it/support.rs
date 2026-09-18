//! Fixtures the merge tests share.
//!
//! ⚠️ **Its own module because two test files need them** (`M5.4`): the merge's
//! happy paths and its refusals are different subjects, and `merge.rs` reached
//! `code-structure.md`'s 500-line limit holding both plus the store double.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]
// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module, and a test-binary submodule is exactly that.
// `pub(crate)` is the visibility that is actually true of these fixtures —
// two sibling test modules use them and nothing else can — so the lint that
// disagrees is the one allowed. `oqueue-core`'s `bundle.rs` makes the same
// call for the same reason.
#![allow(clippy::redundant_pub_crate)]

use oqueue_compact::{CompactionPlan, Planning, plan};
use oqueue_core::{
    BundleBuilder, ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, FakeObjectStore,
    MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey, ObjectRef, ObjectStore, Offset,
    PartitionId, PushedRecords, Result, TAIL_WINDOW_ENTRIES, TopicId,
};
use std::sync::atomic::{AtomicUsize, Ordering};

/// One object's commit at version `version` — the record every fixture here
/// builds, in one place, so a field the log grows (`M5.86` added a timestamp)
/// is one edit rather than one per fixture.
pub(crate) const fn committed(
    version: u64,
    object: ObjectKey,
    spans: Vec<CommittedSpan>,
) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object,
            spans,
            written_at: oqueue_core::Timestamp::EPOCH,
        },
    )
}

pub(crate) fn topic() -> TopicId {
    TopicId::new("t").expect("a valid topic")
}

pub(crate) fn other() -> TopicId {
    TopicId::new("u").expect("a valid topic")
}

pub(crate) fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

pub(crate) fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

pub(crate) fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a valid key")
}

/// A store that counts reads and how many are ever in flight at once.
#[derive(Debug)]
pub(crate) struct Counting {
    pub(crate) inner: FakeObjectStore,
    pub(crate) gets: AtomicUsize,
    pub(crate) puts: AtomicUsize,
    pub(crate) in_flight: AtomicUsize,
    pub(crate) peak_in_flight: AtomicUsize,
    pub(crate) peak_bytes: AtomicUsize,
}

impl Counting {
    pub(crate) fn new() -> Self {
        Self {
            inner: FakeObjectStore::new(),
            gets: AtomicUsize::new(0),
            puts: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            peak_in_flight: AtomicUsize::new(0),
            peak_bytes: AtomicUsize::new(0),
        }
    }
}

impl ObjectStore for Counting {
    fn get<'a>(
        &'a self,
        key: &'a ObjectKey,
        range: ByteRange,
    ) -> oqueue_core::BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.gets.fetch_add(1, Ordering::Relaxed);
            let now = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
            self.peak_in_flight.fetch_max(now, Ordering::Relaxed);
            let bytes = self.inner.get(key, range).await;
            if let Ok(payload) = &bytes {
                self.peak_bytes.fetch_max(payload.len(), Ordering::Relaxed);
            }
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            bytes
        })
    }

    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<oqueue_core::Precondition>,
    ) -> oqueue_core::BoxFuture<'a, Result<oqueue_core::ObjectMeta>> {
        self.puts.fetch_add(1, Ordering::Relaxed);
        self.inner.put(key, payload, precondition)
    }

    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> oqueue_core::BoxFuture<'a, Result<Box<dyn oqueue_core::MultipartWriter<'a> + 'a>>> {
        self.puts.fetch_add(1, Ordering::Relaxed);
        self.inner.open_multipart(key)
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> oqueue_core::BoxFuture<'a, Result<()>> {
        self.inner.delete(keys)
    }
}

/// Writes one object holding `count` records of `size` bytes for `t`.
pub(crate) async fn write_input(
    store: &Counting,
    name: &str,
    t: &TopicId,
    count: u32,
    size: usize,
) {
    let mut builder = BundleBuilder::new();
    builder
        .push(
            t.clone(),
            partition(),
            PushedRecords {
                count,
                producer: None,
            },
            &vec![b'r'; size],
        )
        .expect("a valid region");
    let sealed = builder.seal().expect("a sealed bundle");
    store
        .inner
        .put(&key(name), sealed.into_payload(), None)
        .await
        .expect("a store that accepts");
}

/// `count` objects of `per` records each, written and referenced.
pub(crate) async fn inputs_of(
    store: &Counting,
    count: usize,
    per: u32,
    size: usize,
) -> Vec<ObjectRef> {
    let mut refs = Vec::new();
    for i in 0..count {
        let name = format!("in-{i}");
        write_input(store, &name, &topic(), per, size).await;
        let base = offset(i64::try_from(i).expect("a small count") * i64::from(per));
        refs.push(ObjectRef::new(key(&name), base, per));
    }
    store.gets.store(0, Ordering::Relaxed);
    refs
}

/// A plan over `[0, end)` for the partition these fixtures write.
pub(crate) fn planned(end: i64) -> CompactionPlan {
    planned_from(0, end)
}

/// A plan over `objects` objects of `per` records each.
pub(crate) fn planned_records(per: u32, objects: usize) -> CompactionPlan {
    let index = FakeMaterializedIndex::new();
    let mut counts = vec![per; objects];
    counts.extend(std::iter::repeat_n(per, TAIL_WINDOW_ENTRIES));
    let entries: Vec<MetadataEntry> = counts
        .iter()
        .enumerate()
        .map(|(i, count)| {
            committed(
                i as u64 + 1,
                key(&format!("planned-{i}")),
                vec![CommittedSpan::new(
                    topic(),
                    partition(),
                    *count,
                    ByteRange::bounded(0, u64::from(*count)).expect("a valid range"),
                    None,
                )],
            )
        })
        .collect();
    index.apply(&entries).expect("a valid fold");
    let end = i64::from(per) * i64::try_from(objects).expect("a small count");
    let Planning::Planned(planned) =
        plan(&index, &topic(), partition(), offset(0), offset(end)).expect("an index that answers")
    else {
        panic!("a plan over an amplified, aged range")
    };
    planned
}

/// A plan over `[start, end)`, against an index of one-record objects.
pub(crate) fn planned_from(start: i64, end: i64) -> CompactionPlan {
    let index = FakeMaterializedIndex::new();
    let per = 1_u32;
    let objects = usize::try_from(end).expect("a small count");
    let mut counts = vec![per; objects];
    counts.extend(std::iter::repeat_n(per, TAIL_WINDOW_ENTRIES));
    let entries: Vec<MetadataEntry> = counts
        .iter()
        .enumerate()
        .map(|(i, count)| {
            committed(
                i as u64 + 1,
                key(&format!("planned-{i}")),
                vec![CommittedSpan::new(
                    topic(),
                    partition(),
                    *count,
                    ByteRange::bounded(0, u64::from(*count)).expect("a valid range"),
                    None,
                )],
            )
        })
        .collect();
    index.apply(&entries).expect("a valid fold");
    let Planning::Planned(planned) =
        plan(&index, &topic(), partition(), offset(start), offset(end))
            .expect("an index that answers")
    else {
        panic!("a plan over an amplified, aged range")
    };
    planned
}

/// A partition by index.
pub(crate) fn partition_n(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

/// `count` objects of `per` records each for one topic and partition.
pub(crate) async fn inputs_of_topic(
    store: &Counting,
    at: (&TopicId, PartitionId),
    count: usize,
    per: u32,
    size: usize,
) -> Vec<ObjectRef> {
    let (topic, partition) = at;
    let mut refs = Vec::new();
    for i in 0..count {
        let name = format!("{}-{}-{i}", topic.as_str(), partition.get());
        let mut builder = BundleBuilder::new();
        builder
            .push(
                topic.clone(),
                partition,
                PushedRecords {
                    count: per,
                    producer: None,
                },
                // ⚠️ **One byte per topic, so "and nothing else" can fail.**
                // A shared filler makes a foreign region inside a span
                // indistinguishable from this partition's own bytes, which is
                // the assertion a layout test most needs to be able to fail.
                &vec![fill_for(topic); size],
            )
            .expect("a valid region");
        let sealed = builder.seal().expect("a sealed bundle");
        store
            .inner
            .put(&key(&name), sealed.into_payload(), None)
            .await
            .expect("a store that accepts");
        refs.push(ObjectRef::new(
            key(&name),
            offset(i64::try_from(i).expect("a small count") * i64::from(per)),
            per,
        ));
    }
    refs
}

/// A plan over `[0, end)` for one topic and partition.
pub(crate) fn planned_topic(topic: &TopicId, partition: PartitionId, end: i64) -> CompactionPlan {
    planned_records_topic(
        topic,
        partition,
        1,
        usize::try_from(end).expect("a small count"),
    )
}

/// A plan over `objects` objects of `per` records each, for one topic.
pub(crate) fn planned_records_topic(
    topic: &TopicId,
    partition: PartitionId,
    per: u32,
    objects: usize,
) -> CompactionPlan {
    let index = FakeMaterializedIndex::new();
    let end = i64::from(per) * i64::try_from(objects).expect("a small count");
    let mut counts = vec![per; objects];
    counts.extend(std::iter::repeat_n(per, TAIL_WINDOW_ENTRIES));
    let entries: Vec<MetadataEntry> = counts
        .iter()
        .enumerate()
        .map(|(i, count)| {
            committed(
                i as u64 + 1,
                key(&format!("planned-{i}")),
                vec![CommittedSpan::new(
                    topic.clone(),
                    partition,
                    *count,
                    ByteRange::bounded(0, u64::from(*count)).expect("a valid range"),
                    None,
                )],
            )
        })
        .collect();
    index.apply(&entries).expect("a valid fold");
    let Planning::Planned(planned) =
        plan(&index, topic, partition, offset(0), offset(end)).expect("an index that answers")
    else {
        panic!("a plan over an amplified, aged range")
    };
    planned
}

/// The byte `inputs_of_topic` fills a topic's records with.
pub(crate) fn fill_for(topic: &TopicId) -> u8 {
    topic.as_str().as_bytes()[0]
}

/// `count` one-record objects for a topic and partition, based at `from`.
pub(crate) async fn shifted_inputs(
    store: &Counting,
    at: (&TopicId, PartitionId),
    count: usize,
    from: i64,
) -> Vec<ObjectRef> {
    let (topic, partition) = at;
    let mut refs = Vec::new();
    for i in 0..count {
        let name = format!("{}-{}-shifted-{i}", topic.as_str(), partition.get());
        let mut builder = BundleBuilder::new();
        builder
            .push(
                topic.clone(),
                partition,
                PushedRecords {
                    count: 1,
                    producer: None,
                },
                &[fill_for(topic); 32],
            )
            .expect("a valid region");
        let sealed = builder.seal().expect("a sealed bundle");
        store
            .inner
            .put(&key(&name), sealed.into_payload(), None)
            .await
            .expect("a store that accepts");
        refs.push(ObjectRef::new(
            key(&name),
            offset(from + i64::try_from(i).expect("a small count")),
            1,
        ));
    }
    refs
}

/// A plan over `[start, end)` for one topic and partition.
pub(crate) fn planned_from_topic(
    topic: &TopicId,
    partition: PartitionId,
    start: i64,
    end: i64,
) -> CompactionPlan {
    let index = FakeMaterializedIndex::new();
    let objects = usize::try_from(end).expect("a small count");
    let mut counts = vec![1_u32; objects];
    counts.extend(std::iter::repeat_n(1_u32, TAIL_WINDOW_ENTRIES));
    let entries: Vec<MetadataEntry> = counts
        .iter()
        .enumerate()
        .map(|(i, count)| {
            committed(
                i as u64 + 1,
                key(&format!("planned-{i}")),
                vec![CommittedSpan::new(
                    topic.clone(),
                    partition,
                    *count,
                    ByteRange::bounded(0, u64::from(*count)).expect("a valid range"),
                    None,
                )],
            )
        })
        .collect();
    index.apply(&entries).expect("a valid fold");
    let Planning::Planned(planned) =
        plan(&index, topic, partition, offset(start), offset(end)).expect("an index that answers")
    else {
        panic!("a plan over an amplified, aged range")
    };
    planned
}

// ⚠️ The index-side fixtures live in `indexes.rs` — `support.rs` reached the
// 500-line limit holding both halves (`code-structure.md` rule 18). Re-exported
// so every caller keeps the one import path.
pub(crate) use crate::indexes::{CountingIndex, history_index};
