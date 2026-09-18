//! What a retention round reads out of the index: a partition's age and
//! where its readable log begins.
//!
//! ⚠️ **Its own module because these are retention's questions, not the
//! fold's** (`code-structure.md` rule 18). Both answer without touching object
//! storage, which is the property a round asked of every partition a node
//! holds needs — `ADR-0036` decision 1 gives compaction's trigger the same.

use super::IndexState;
use crate::{Offset, PartitionId, TimeSpan, TopicId};

impl IndexState {
    /// The first readable offset, zero until a trim moves it (`M5.19`).
    #[must_use]
    pub fn log_start(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.partition(topic, partition)
            .map_or(Offset::ZERO, |p| p.log_start)
    }

    /// When this partition's commits happened, if any have.
    ///
    /// ⚠️ **What FR-33's decision reads, and it costs no object-storage
    /// operation** (`M5.86`). A retention round asks this of every partition a
    /// node holds, so a decision that reached for an object would be one GET
    /// per partition per round — `ADR-0036` decision 1 is the same property
    /// one requirement over, and compaction's trigger already obeys it.
    ///
    /// ⚠️ **`None` is "nothing has been committed here", not "committed at the
    /// epoch".** A partition the fold knows about only because a manifest was
    /// published for it has no commit time of its own, and a round reading
    /// `EPOCH` there would reap it on its first sweep.
    #[must_use]
    pub fn time_span(&self, topic: &TopicId, partition: PartitionId) -> Option<TimeSpan> {
        self.partition(topic, partition).and_then(|slot| slot.when)
    }
}
