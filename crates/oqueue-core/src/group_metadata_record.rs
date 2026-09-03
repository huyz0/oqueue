//! What one entry in the group metadata log is — `M4.14`, `ADR-0035`.

use crate::{GroupId, TopicId};

/// One event `GroupMetadataLog` durably records.
///
/// ⚠️ **Exhaustive, deliberately** — `oqueue_core::MetadataRecord`'s own
/// module doc names the reason and it applies here unchanged: a new variant
/// is a new event every applier must be made to consider, and a `_ =>` arm
/// absorbing it silently is exactly the bug that would not surface until a
/// replay produced the wrong offset.
///
/// ⚠️ **Its own type, not a `MetadataRecord` variant** — `ADR-0035`. The two
/// logs are unrelated seams; folding this into the topic metadata log's own
/// exhaustive match would force every partition-offset applier to consider
/// an event that means nothing to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupMetadataRecord {
    /// A group committed an offset for one `(topic, partition)`.
    OffsetCommitted {
        /// The committing group.
        group: GroupId,
        /// The topic the offset belongs to.
        topic: TopicId,
        /// The partition the offset belongs to.
        partition: i32,
        /// The committed offset.
        offset: i64,
    },
}
