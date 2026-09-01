//! The single monotonic allocator — the sans-I/O half of assign → journal → ack.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` contradict each other on an
// item in a private module: the first wants `pub`, the second wants
// `pub(crate)` back. `pub(crate)` is the visibility that is actually true —
// nothing here crosses the crate boundary — so it stays and the lint that
// disagrees is the one allowed. Same trade, same reason, as `oqueue-core`'s
// `test_executor`.
#![allow(clippy::redundant_pub_crate)]

mod admission;

// ⚠️ `RejectReason` is `pub` (`M11.6`) so `oqueue-broker` can name and match
// on it through `SpanOutcome::Rejected`, but `admission` itself stays
// private — nothing outside this crate needs the module, only the one type
// this re-export and `lib.rs`'s own re-export of *this* path expose.
pub use admission::RejectReason;

use crate::commit::Assignment;
use oqueue_core::{
    CommitVersion, CommittedSpan, MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId,
    ProducerEpoch, ProducerId, Result, TopicId,
};
use std::collections::HashMap;

/// What `stage` recorded the last time this `(producer, topic, partition)`
/// committed — read to decide the next commit, never mutated until a journal
/// append actually lands (`ADR-0031` point 1).
///
/// ⚠️ **Fields readable by `allocator::admission` without accessors.** A
/// child module sees its parent's private items by Rust's own visibility
/// rule, and a getter for each of four fields used only inside this crate
/// would be ceremony `admission.rs`'s own doc explains is unneeded.
#[derive(Debug, Clone, Copy)]
struct ProducerState {
    epoch: ProducerEpoch,
    sequence: i32,
    base_offset: Offset,
    record_count: u32,
}

impl ProducerState {
    /// An admitted span's state, before its real offset is known.
    ///
    /// ⚠️ `base_offset: Offset::ZERO` is a placeholder `admission.rs`'s own
    /// `running` map never reads for its offset — only `stage` assigns the
    /// real one, staging the corrected value `apply` actually persists.
    const fn provisional(epoch: ProducerEpoch, sequence: i32, record_count: u32) -> Self {
        Self {
            epoch,
            sequence,
            base_offset: Offset::ZERO,
            record_count,
        }
    }
}

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
    producer_ends: Vec<((ProducerId, TopicId, PartitionId), ProducerState)>,
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
    producer_state: HashMap<(ProducerId, TopicId, PartitionId), ProducerState>,
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
            .field("producers", &self.producer_state.len())
            .finish()
    }
}

impl Allocator {
    /// A fresh line: version zero, every partition at [`Offset::ZERO`], no
    /// producer has been seen.
    pub(crate) fn new() -> Self {
        Self {
            next_version: CommitVersion::ZERO,
            end_offsets: HashMap::new(),
            producer_state: HashMap::new(),
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
        // ⚠️ **Every span here already passed `admit`** — the caller's
        // responsibility, not this function's to re-check — so a span
        // carrying a producer identity is, by construction, either the
        // first sequence a line has ever seen or genuinely the next one.
        // What is computed here is *where* that sequence lands, which
        // `admit` could not know without assigning the offset itself.
        let mut producer_ends: Vec<((ProducerId, TopicId, PartitionId), ProducerState)> =
            Vec::new();

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
            if let Some(identity) = span.producer() {
                producer_ends.push((
                    (identity.id(), span.topic().clone(), span.partition()),
                    ProducerState {
                        epoch: identity.epoch(),
                        sequence: identity.sequence(),
                        base_offset: base,
                        record_count: span.record_count(),
                    },
                ));
            }
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
            producer_ends,
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
        for (key, state) in staged.producer_ends {
            self.producer_state.insert(key, state);
        }
        self.next_version = staged.next_version;
        (staged.entry.version(), staged.assignments)
    }

    /// An allocator already near the end of a line, for the tests that are
    /// about what happens when one runs out.
    ///
    /// ⚠️ **`cfg(test)`, and it is the only way those guards are reachable.**
    /// `stage`'s two refusals need a partition within one batch of `i64::MAX`
    /// records, or a shard within one commit of `u64::MAX` — and neither
    /// ceiling is a named constant, because both are the range of the integer
    /// itself, reached through `Offset::add`'s and `CommitVersion::advance`'s
    /// `checked_add`. ⚠️ **Getting there by producing is not a fixture, it is
    /// a geological era**: `record_count` is a `u32`, so even a request
    /// carrying the largest batch the format can express needs upwards of two
    /// billion of them, and a realistic one needs orders more. Both are the arithmetic that keeps a
    /// wrapped offset from being *smaller* than the one before it, and every
    /// later comparison wrong, so leaving them unreachable would mean the code
    /// standing between a client and silently reordered offsets was the code
    /// nothing ever ran.
    ///
    /// ⚠️ **Seeds state, does not fake behaviour** (`testing.md` rule 3): what
    /// it produces is an ordinary `Allocator` that has been running for a long
    /// time, and every method behaves exactly as it would have.
    #[cfg(test)]
    pub(crate) fn seeded(
        next_version: CommitVersion,
        ends: &[(TopicId, PartitionId, Offset)],
    ) -> Self {
        let mut allocator = Self::new();
        allocator.next_version = next_version;
        for (topic, partition, end) in ends {
            allocator
                .end_offsets
                .entry(topic.clone())
                .or_default()
                .insert(*partition, *end);
        }
        allocator
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
    use oqueue_core::{
        ByteRange, CommitVersion, CommittedSpan, Error, ObjectKey, Offset, PartitionId, TopicId,
    };

    fn topic() -> TopicId {
        TopicId::new("t".to_owned()).expect("a valid topic id")
    }

    fn partition() -> PartitionId {
        PartitionId::new(0).expect("a valid partition")
    }

    fn span(records: u32) -> CommittedSpan {
        CommittedSpan::new(topic(), partition(), records, ByteRange::Full, None)
    }

    fn object() -> ObjectKey {
        ObjectKey::new("o".to_owned()).expect("a valid object key")
    }

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
            None,
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

    /// ⚠️ **A partition's line refuses rather than wrapping.** An offset is an
    /// `i64` on the wire, and a wrapped one is *smaller* than the one before
    /// it — every later comparison, every watermark, every consumer's position
    /// is then wrong, and nothing tells anybody. `Offset::add` is what refuses;
    /// this is what reaches it, which two-billion produce requests otherwise
    /// would.
    #[test]
    fn a_partition_at_the_end_of_its_line_refuses_rather_than_wrapping() {
        let allocator = Allocator::seeded(
            CommitVersion::ZERO,
            &[(
                topic(),
                partition(),
                Offset::new(i64::MAX - 1).expect("in range"),
            )],
        );

        let refused = allocator.stage(object(), vec![span(2)]);

        assert!(
            matches!(refused, Err(Error::OffsetOverflow { .. })),
            "a line one record from its end must refuse two: {refused:?}"
        );
        // ⚠️ **And the one that fits still fits**, which is what says this is a
        // ceiling rather than an off-by-one: a guard one too eager refuses the
        // last legal record of every partition that ever reaches here.
        allocator
            .stage(object(), vec![span(1)])
            .expect("the last record on the line is still assignable");
    }

    /// ⚠️ **And the shard's version line does the same.** A wrapped
    /// `CommitVersion` is a version *below* one already folded, which
    /// `MaterializedIndex` guarantee 1 refuses as non-monotonic — so the shard
    /// stops committing with a message about ordering rather than about the
    /// counter that ran out.
    #[test]
    fn a_version_line_at_its_end_refuses_rather_than_wrapping() {
        let allocator = Allocator::seeded(CommitVersion::new(u64::MAX), &[]);

        let refused = allocator.stage(object(), vec![span(1)]);

        assert!(
            matches!(refused, Err(Error::CommitVersionOverflow { .. })),
            "the last version on the line cannot be advanced past: {refused:?}"
        );
        // ⚠️ **And the last legal version still stages**, the same companion
        // the offset test carries and for the same reason: a guard one step
        // too eager costs a shard its final commit, and a test that only ever
        // probes the value past the end cannot tell the two apart.
        Allocator::seeded(CommitVersion::new(u64::MAX - 1), &[])
            .stage(object(), vec![span(1)])
            .expect("the last version on the line is still assignable");
    }

    /// ⚠️ **Nothing is consumed by a refusal**, which is what makes both
    /// guards safe to hit: `stage` computes without taking, so a caller that
    /// retries a smaller batch gets the offsets it would have got anyway.
    #[test]
    fn a_refused_stage_consumes_nothing() {
        let allocator = Allocator::seeded(
            CommitVersion::new(7),
            &[(
                topic(),
                partition(),
                Offset::new(i64::MAX - 1).expect("in range"),
            )],
        );

        assert!(allocator.stage(object(), vec![span(2)]).is_err());

        let staged = allocator
            .stage(object(), vec![span(1)])
            .expect("a batch that fits still fits");
        assert_eq!(staged.entry.version(), CommitVersion::new(7));
    }
}
