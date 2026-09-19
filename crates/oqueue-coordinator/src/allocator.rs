//! The single monotonic allocator — the sans-I/O half of assign → journal → ack.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` contradict each other on an
// item in a private module: the first wants `pub`, the second wants
// `pub(crate)` back. `pub(crate)` is the visibility that is actually true —
// nothing here crosses the crate boundary — so it stays and the lint that
// disagrees is the one allowed. Same trade, same reason, as `oqueue-core`'s
// `test_executor`.
#![allow(clippy::redundant_pub_crate)]

mod admission;
mod expiry;

// ⚠️ `RejectReason` is `pub` (`M11.6`) so `oqueue-broker` can name and match
// on it through `SpanOutcome::Rejected`, but `admission` itself stays
// private — nothing outside this crate needs the module, only the one type
// this re-export and `lib.rs`'s own re-export of *this* path expose.
pub use admission::RejectReason;

use crate::commit::Assignment;
use oqueue_core::{
    CommitVersion, CommittedSpan, MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId,
    ProducerEpoch, ProducerId, Result, Timestamp, TopicId,
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
    /// Which key in `producer_state` was touched least recently, so `apply`
    /// can evict one in `O(log n)` once `producer_state` is at
    /// [`expiry::MAX_TRACKED_PRODUCERS`] — `M11.8`, `ADR-0031` point 6.
    producer_recency: expiry::Recency<(ProducerId, TopicId, PartitionId)>,
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
            // ⚠️ `producer_recency` is deliberately absent, not merely
            // unlisted: it names the same per-tenant keys `producer_state`
            // already withholds, and its size always equals `producers`
            // above, so listing it too would be the redundant half of the
            // same leak. `finish_non_exhaustive` says so explicitly rather
            // than looking like every field was named.
            .finish_non_exhaustive()
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
            producer_recency: expiry::Recency::default(),
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
    pub(crate) fn stage(
        &self,
        object: ObjectKey,
        spans: Vec<CommittedSpan>,
        written_at: Timestamp,
    ) -> Result<Staged> {
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
                MetadataRecord::BatchCommitted {
                    object,
                    spans,
                    written_at,
                },
            ),
            assignments,
            ends,
            producer_ends,
            next_version: self.next_version.advance(1)?,
        })
    }

    /// Folds one entry already in the log, as the commit that wrote it did
    /// (`M6.3`).
    ///
    /// ⚠️ **The same `stage` and `apply` the live path runs**, so a replayed
    /// line cannot drift from the one that was served: offsets, the version
    /// line, and producer sequence state all come back by the arithmetic that
    /// produced them. A version above the next one is taken as the next —
    /// the log is monotonic, not necessarily contiguous — and one below it is
    /// a log this allocator has already folded past.
    ///
    /// # Errors
    ///
    /// [`Error::NonMonotonicCommitVersion`](oqueue_core::Error::NonMonotonicCommitVersion)
    /// for an entry below the next version; whatever `stage` refuses for a
    /// commit the live path would have refused.
    pub(crate) fn replay(&mut self, entry: &MetadataEntry) -> Result<()> {
        if entry.version() < self.next_version {
            return Err(oqueue_core::Error::NonMonotonicCommitVersion {
                expected_above: self.next_version.get().saturating_sub(1),
                got: entry.version().get(),
            });
        }
        self.next_version = entry.version();
        match entry.record() {
            MetadataRecord::BatchCommitted {
                object,
                spans,
                written_at,
            } => {
                let staged = self.stage(object.clone(), spans.clone(), *written_at)?;
                self.apply(staged);
            }
            // ⚠️ Every other record takes a version and moves no offset — a
            // trim included, whose validity the index's own fold checks.
            _ => self.next_version = self.next_version.advance(1)?,
        }
        Ok(())
    }

    /// Stages a trim of one partition to `start`, taking a version and no
    /// offsets (`M5.90`).
    ///
    /// ⚠️ **Refused here when it is past the partition's end**, the check the
    /// fold makes (`M5.19`) made before the journal instead of after it: a
    /// `Trimmed` the fold refuses is refused identically on every replay, so
    /// journaling one would leave a log that no index can fold past.
    ///
    /// # Errors
    ///
    /// [`Error::TrimPastEnd`](oqueue_core::Error::TrimPastEnd) if `start` is
    /// past the partition's end, and
    /// [`Error::CommitVersionOverflow`](oqueue_core::Error::CommitVersionOverflow)
    /// if the version line would leave its range.
    pub(crate) fn stage_trim(
        &self,
        topic: TopicId,
        partition: PartitionId,
        start: Offset,
    ) -> Result<Staged> {
        let end = self.end_offset(&topic, partition).unwrap_or(Offset::ZERO);
        if start > end {
            return Err(oqueue_core::Error::TrimPastEnd {
                topic: topic.to_string(),
                partition: partition.get(),
                start: start.get(),
                end: end.get(),
            });
        }
        Ok(Staged {
            entry: MetadataEntry::new(
                self.next_version,
                MetadataRecord::Trimmed {
                    topic,
                    partition,
                    start,
                },
            ),
            assignments: Vec::new(),
            ends: Vec::new(),
            producer_ends: Vec::new(),
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
            self.producer_recency.touch(key.clone());
            self.producer_state.insert(key, state);
        }
        evict_over_cap(
            &mut self.producer_state,
            &mut self.producer_recency,
            expiry::MAX_TRACKED_PRODUCERS,
        );
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

/// Evicts from `producer_state`, guided by `recency`, until at most `cap`
/// entries remain — `M11.8`, `ADR-0031` point 6.
///
/// ⚠️ **`cap` is a parameter, not `expiry::MAX_TRACKED_PRODUCERS` read
/// directly, so a test can drive this against a small number rather than
/// reaching the real cap through 100,000 real commits** — `admission.rs`'s
/// own `next_sequence` boundary test drives that function directly rather
/// than through two billion real ones, on the same reasoning.
fn evict_over_cap(
    producer_state: &mut HashMap<(ProducerId, TopicId, PartitionId), ProducerState>,
    recency: &mut expiry::Recency<(ProducerId, TopicId, PartitionId)>,
    cap: usize,
) {
    // ⚠️ A `while`, not an `if`: a caller may have inserted more than one
    // entry past the cap in one call (`apply` folds a whole commit's worth
    // of spans before this runs), and each eviction removes only one.
    while producer_state.len() > cap {
        let Some(oldest) = recency.evict_oldest() else {
            break;
        };
        producer_state.remove(&oldest);
    }
}

#[cfg(test)]
mod tests;
