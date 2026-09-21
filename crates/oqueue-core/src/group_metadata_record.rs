//! What one entry in the group metadata log is — `M4.14`, `ADR-0035`;
//! `M4.15c` adds the second variant, `GroupTransitioned`.

use crate::{GroupEvent, GroupId, GroupRosterSnapshot, TopicId};

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
    /// A group's own `GroupCoordinator` state machine accepted `event` —
    /// `M4.15c`. Replayed by re-firing `event` through
    /// [`crate::GroupCoordinator::transition`] in the order these records
    /// were appended, `GroupState::transition`'s own determinism (doc 02
    /// §3.1) making that equivalent to what actually happened live —
    /// `crate::GroupState`'s own doc names the same "only the legal
    /// transitions, nothing else" guarantee replay leans on here.
    GroupTransitioned {
        /// The group the event applied to.
        group: GroupId,
        /// The event `GroupState::transition` accepted.
        event: GroupEvent,
    },
    /// The latest non-sensitive roster summary for administrative reads.
    GroupRosterUpdated {
        /// The group whose summary changed.
        group: GroupId,
        /// The new summary, or `None` after the group loses all members.
        roster: Option<GroupRosterSnapshot>,
    },
}
