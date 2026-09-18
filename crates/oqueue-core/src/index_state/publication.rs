//! Judging a batch's manifest publications, and applying them.
//!
//! ⚠️ **Its own module because admitting a publication and folding a commit
//! are two concepts** (`code-structure.md` rule 18), and `index_state.rs`
//! reached the 500-line limit holding both. That file is the fold over the
//! log; this is the question a publication has to answer before the fold
//! touches anything — which boundary it must meet, and how much of its own
//! batch it is allowed to see when answering.

use crate::{Error, ObjectKey, Offset, PartitionId, Result, TailEntry, TopicId};
use std::collections::HashMap;

use super::{IndexState, partition::PartitionIndex};

/// One `ManifestPublished`, and how much of its own batch it may see.
///
/// ⚠️ **A struct rather than a tuple** because the fifth field is the one that
/// carries the correctness: without it the whole batch is visible to every
/// publication in it, including commits the publication predates.
#[derive(Debug)]
pub(super) struct Publication<'a> {
    pub(super) topic: &'a TopicId,
    pub(super) partition: PartitionId,
    pub(super) manifest: &'a ObjectKey,
    pub(super) upto: Offset,
    /// How many of this partition's staged entries precede it.
    pub(super) precedes: usize,
}

impl<'a> Publication<'a> {
    /// Reads one publication out of a batch, against what the batch has
    /// staged for its partition *so far*.
    ///
    /// ⚠️ **So far, not in total.** A commit arriving after this publication
    /// in the log is not part of the state it was written against, and
    /// counting one widens the tail window — which is what decides how far a
    /// manifest may reach. Counting the whole batch made the same log fold one
    /// way whole and another way entry-at-a-time, with one broker
    /// materializing the index and another refusing it. `M5.62`'s fourth
    /// round measured it.
    pub(super) fn staged(
        topic: &'a TopicId,
        partition: PartitionId,
        manifest: &'a ObjectKey,
        upto: Offset,
        staged: &HashMap<(&TopicId, PartitionId), (Offset, Vec<TailEntry>)>,
    ) -> Self {
        Self {
            topic,
            partition,
            manifest,
            upto,
            precedes: staged
                .get(&(topic, partition))
                .map_or(0, |(_, entries)| entries.len()),
        }
    }
}

impl IndexState {
    /// Refuses a batch whose publications do not meet the history they
    /// replace, before anything is mutated — guarantee 2.
    ///
    /// ⚠️ **It reads the staged entries that precede each publication**, so a
    /// publication arriving in the same batch as the commits it covers sees
    /// them. A replay pages the log in arbitrary chunks, and a fold that
    /// refused a page holding both could not replay one. ⚠️ **Preceding, not
    /// all of them**: a commit after the publication in the log is not part of
    /// the state it was written against, and reading one made a whole page
    /// accept what the same log folded one entry at a time refused.
    ///
    /// # Errors
    ///
    /// [`Error::ManifestDoesNotMeetHistory`] naming the boundary above the one
    /// claimed, and [`Error::OffsetOverflow`] if an entry's own fold was
    /// wrong.
    pub(super) fn admissible(
        &self,
        published: &[Publication<'_>],
        staged: &HashMap<(&TopicId, PartitionId), (Offset, Vec<TailEntry>)>,
    ) -> Result<()> {
        let empty: Vec<TailEntry> = Vec::new();
        let fresh_default = PartitionIndex::default();
        // ⚠️ **A running boundary per partition, so a batch holding two
        // publications is judged exactly as two batches would judge them.**
        // Keeping only the last was tried and made the fold's result depend on
        // how the log was paged: one page accepted a descending pair and two
        // pages refused it, so two brokers rebuilding the same log disagreed
        // about whether it could be materialized at all. `M5.62`'s second
        // round measured it.
        let mut covered: HashMap<(&TopicId, PartitionId), Offset> = HashMap::new();
        for publication in published {
            let (topic, partition, upto) =
                (publication.topic, publication.partition, publication.upto);
            let fresh = staged
                .get(&(topic, partition))
                .map_or(empty.as_slice(), |(_, entries)| {
                    &entries[..publication.precedes]
                });
            let slot = self.partition(topic, partition).unwrap_or(&fresh_default);
            // ⚠️ **Non-decreasing, and a decrease is corrupt rather than
            // ignorable.** A manifest covering less than the one before it
            // un-covers records that are already in an object nothing else
            // names, and no compaction produces one — so it is refused in
            // whatever page it arrives in.
            // ⚠️ **Only this batch's running value, not the slot's own.** A
            // publication below one the index has *already applied* is caught
            // by the boundary check instead: absorbing removes every history
            // base below the covered offset, and a manifest may not reach past
            // history, so a later, smaller `upto` is no longer a boundary.
            // Reading the slot here as well was a second path to the same
            // refusal, and no input could tell it from the first.
            if let Some(already) = covered.get(&(topic, partition)).copied()
                && upto < already
            {
                return Err(Error::ManifestDoesNotMeetHistory {
                    upto: upto.get(),
                    expected: already.get(),
                });
            }
            if let Some(expected) = slot.boundary(upto, fresh) {
                return Err(Error::ManifestDoesNotMeetHistory {
                    upto: upto.get(),
                    expected: expected.get(),
                });
            }
            covered.insert((topic, partition), upto);
        }
        Ok(())
    }

    /// Replaces each published partition's covered history with one reference.
    ///
    /// ⚠️ **It creates the partition rather than skipping it.** A publication
    /// for a partition the index has never seen is admissible — a fresh slot
    /// ends at offset zero and a manifest covering `[0, 0)` meets it — so
    /// dropping it made the fold depend on paging again: fold the publication
    /// together with the commits that follow it and the commits create the
    /// slot first, so the manifest lands; fold it alone and there is no slot,
    /// so it is discarded and `entries()` is one short forever after. Same
    /// log, same accept decision, two states. `M5.62`'s fifth round.
    pub(super) fn absorb_published(&mut self, published: Vec<Publication<'_>>) {
        for publication in published {
            let slot = self
                .partitions
                .entry(publication.topic.clone())
                .or_default()
                .entry(publication.partition)
                .or_default();
            let (removed, superseded) = slot.absorb(publication.manifest.clone(), publication.upto);
            // ⚠️ **The reference is itself an entry**, so the first manifest a
            // partition gets costs one and every later one supersedes rather
            // than joins: the count moves by the entries absorbed, minus the
            // one this reference adds when there was not already one.
            self.entries -= removed;
            if !superseded {
                self.entries += 1;
            }
        }
    }
}
