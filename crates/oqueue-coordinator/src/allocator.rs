//! The single monotonic allocator — the sans-I/O half of assign → journal → ack.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` contradict each other on an
// item in a private module: the first wants `pub`, the second wants
// `pub(crate)` back. `pub(crate)` is the visibility that is actually true —
// nothing here crosses the crate boundary — so it stays and the lint that
// disagrees is the one allowed. Same trade, same reason, as `oqueue-core`'s
// `test_executor`.
#![allow(clippy::redundant_pub_crate)]

use crate::commit::Assignment;
use oqueue_core::{
    CommitVersion, CommittedSpan, MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId,
    Result, TopicId,
};
use std::collections::HashMap;

/// A commit that has been assigned a position but not yet journaled.
///
/// ⚠️ **Everything fallible has already happened by the time one of these
/// exists.** The version line and every partition's offset line are advanced
/// into `next_version`/`ends` here, where a failure costs nothing; applying it
/// afterwards is total. That ordering is what makes a refused append leave the
/// allocator exactly where it was, rather than one commit ahead of a log that
/// never received it.
#[derive(Debug)]
pub(crate) struct Staged {
    entry: MetadataEntry,
    assignments: Vec<Assignment>,
    ends: Vec<(TopicId, PartitionId, Offset)>,
    next_version: CommitVersion,
}

impl Staged {
    /// The record to journal.
    pub(crate) const fn entry(&self) -> &MetadataEntry {
        &self.entry
    }
}

/// One metadata shard's version line and every partition's offset line.
///
/// ⚠️ **Not an [`IndexState`](oqueue_core::IndexState)**, though it folds the
/// same log to the same offsets — and a test pins that they agree. The index
/// materializes what has *already* committed and may be dropped and refilled
/// at any moment; this is the head of the line, it must survive nothing, and it
/// has to be able to compute a position and then *not* take it when the journal
/// refuses. Rollback is what the two do differently, and it is the whole
/// difference between a cache and an allocator.
pub(crate) struct Allocator {
    next_version: CommitVersion,
    end_offsets: HashMap<TopicId, HashMap<PartitionId, Offset>>,
}

/// ⚠️ Renders how far the version line has reached, never the partitions it
/// holds — a `Debug` line in a log or a test failure must not become a listing
/// of every tenant's topics. Same rule, same reason, as `FakeMetadataLog` and
/// `FakeMaterializedIndex` in `oqueue-core`, and it matters more here: this
/// type is reachable through `CoordinatorLoop`, which an operator may well
/// format on a shutdown path.
impl core::fmt::Debug for Allocator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Allocator")
            .field("next_version", &self.next_version)
            .field(
                "partitions",
                &self.end_offsets.values().map(HashMap::len).sum::<usize>(),
            )
            .finish()
    }
}

impl Allocator {
    /// A fresh line: version zero, every partition at [`Offset::ZERO`].
    pub(crate) fn new() -> Self {
        Self {
            next_version: CommitVersion::ZERO,
            end_offsets: HashMap::new(),
        }
    }

    /// Computes the position `spans` would occupy, without taking it.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`](oqueue_core::Error::OffsetOverflow) if a
    /// partition's line would leave the protocol's `i64` range, or
    /// [`Error::CommitVersionOverflow`](oqueue_core::Error::CommitVersionOverflow)
    /// if the shard's version line would leave the `u64` range. ⚠️ In either
    /// case nothing has been consumed, because nothing has been applied.
    pub(crate) fn stage(&self, object: ObjectKey, spans: Vec<CommittedSpan>) -> Result<Staged> {
        let mut assignments = Vec::with_capacity(spans.len());
        // ⚠️ Keyed on a **borrowed** topic, exactly as
        // `IndexState::stage_span` keys the same fold in `oqueue-core`. A
        // tuple key of owned values would cost a `TopicId` clone per lookup,
        // and a linear scan would make this loop quadratic in the number of
        // distinct `(topic, partition)` pairs — on the one task every producer
        // in the shard queues behind, which `async-concurrency.md` rule 3
        // names directly. FR-32 bundles topics into one PUT and NFR-10 puts
        // ~1,000 partitions in a topic, so "how many pairs" has no small bound
        // to lean on.
        let mut running: HashMap<(&TopicId, PartitionId), Offset> = HashMap::new();

        for span in &spans {
            let key = (span.topic(), span.partition());
            // ⚠️ The running value first and the committed line only then,
            // which is what makes a partition named by two spans of one object
            // accumulate rather than restart.
            let base = running
                .get(&key)
                .copied()
                .or_else(|| self.end_offset(span.topic(), span.partition()))
                .unwrap_or(Offset::ZERO);
            // ⚠️ `Offset::add`, not `+`: a wrapped offset is smaller than the
            // one before it, and every later comparison is then wrong.
            let next = base.add(i64::from(span.record_count()))?;
            // ⚠️ Pushed once per span, in the caller's order — the positional
            // correspondence `Coordinator::commit` documents and a bundler
            // attributing offsets back to individual producers depends on.
            assignments.push(Assignment::new(
                span.topic().clone(),
                span.partition(),
                base,
                span.record_count(),
            ));
            running.insert(key, next);
        }

        // The borrows of `spans` end here, before it moves into the record.
        let ends: Vec<(TopicId, PartitionId, Offset)> = running
            .into_iter()
            .map(|((topic, partition), end)| (topic.clone(), partition, end))
            .collect();

        Ok(Staged {
            entry: MetadataEntry::new(
                self.next_version,
                MetadataRecord::BatchCommitted { object, spans },
            ),
            assignments,
            ends,
            next_version: self.next_version.advance(1)?,
        })
    }

    /// Takes the position [`stage`](Self::stage) computed, once it is durable.
    ///
    /// ⚠️ **Infallible, and that is the design.** Every way this could fail was
    /// resolved before the journal step; a fallible apply would mean a record
    /// that is durable in the log but absent from the line derived from it.
    pub(crate) fn apply(&mut self, staged: Staged) -> (CommitVersion, Vec<Assignment>) {
        for (topic, partition, end) in staged.ends {
            self.end_offsets
                .entry(topic)
                .or_default()
                .insert(partition, end);
        }
        self.next_version = staged.next_version;
        (staged.entry.version(), staged.assignments)
    }

    /// Where this partition's line has reached, if the allocator has seen it.
    fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Option<Offset> {
        self.end_offsets
            .get(topic)
            .and_then(|parts| parts.get(&partition))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::Allocator;
    use oqueue_core::{ByteRange, CommittedSpan, ObjectKey, PartitionId, TopicId};

    /// ⚠️ The allocator is reachable through `CoordinatorLoop`, which an
    /// operator may format on an error or shutdown path. Its `Debug` must
    /// therefore not become a listing of every tenant's topics — the rule
    /// `FakeMetadataLog` and `FakeMaterializedIndex` already follow in
    /// `oqueue-core`, and it binds harder here because this one is not a fake.
    #[test]
    fn formatting_the_allocator_summarises_rather_than_lists() {
        let mut allocator = Allocator::new();
        let span = CommittedSpan::new(
            TopicId::new("secret-tenant-topic".to_owned()).expect("a valid topic id"),
            PartitionId::new(0).expect("a valid partition"),
            1,
            ByteRange::Full,
        );
        let staged = allocator
            .stage(
                ObjectKey::new("o".to_owned()).expect("a valid object key"),
                vec![span],
            )
            .expect("a first commit stages");
        allocator.apply(staged);

        let rendered = format!("{allocator:?}");
        assert!(
            !rendered.contains("secret-tenant-topic"),
            "a Debug line must not name a tenant's topics: {rendered}"
        );
        assert!(
            rendered.contains("next_version: CommitVersion(1)"),
            "and must still say where the line has reached: {rendered}"
        );
        assert!(
            rendered.contains("partitions: 1"),
            "and how much it is holding: {rendered}"
        );
    }
}
