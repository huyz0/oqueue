//! What one entry in the metadata log is.

use crate::{
    ByteRange, CoordinatorEpoch, ObjectKey, Offset, PartitionId, ProducerIdentity, TopicId,
};

/// One `(topic, partition)`'s share of a committed object.
///
/// ⚠️ **A count, never a position.** This is the delta shape `M3.md` task 2
/// requires: the span says how many records the object added to a partition,
/// not where they landed. Where they landed is *derived* by applying the log
/// in order, which is what makes the offsets gap-free by construction rather
/// than by an allocator remembering to be careful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedSpan {
    topic: TopicId,
    partition: PartitionId,
    record_count: u32,
    bytes: ByteRange,
    producer: Option<ProducerIdentity>,
}

impl CommittedSpan {
    /// Builds a span.
    ///
    /// ⚠️ **`producer` is `None` for an ordinary, non-idempotent produce**
    /// (`ADR-0031`) — the wire's own `-1` sentinel decodes to `None`, never to
    /// a `ProducerIdentity` with sentinel-shaped fields. A `Some` is what
    /// lets `Allocator::admit` (`oqueue-coordinator`) fold this span's
    /// sequence into `producer_state` on replay; a `None` costs nothing and
    /// leaves no producer state to reconstruct.
    #[must_use]
    pub const fn new(
        topic: TopicId,
        partition: PartitionId,
        record_count: u32,
        bytes: ByteRange,
        producer: Option<ProducerIdentity>,
    ) -> Self {
        Self {
            topic,
            partition,
            record_count,
            bytes,
            producer,
        }
    }

    /// The topic this share belongs to.
    #[must_use]
    pub const fn topic(&self) -> &TopicId {
        &self.topic
    }

    /// The partition this share belongs to.
    #[must_use]
    pub const fn partition(&self) -> PartitionId {
        self.partition
    }

    /// How many records the object added to that partition.
    #[must_use]
    pub const fn record_count(&self) -> u32 {
        self.record_count
    }

    /// Where inside the object this partition's records live.
    ///
    /// ⚠️ **Still a delta, not a position.** This is an offset *within the
    /// object*, fixed at the moment the object was written, and it says
    /// nothing about where in the partition's log the records landed — that
    /// is derived by folding [`record_count`](Self::record_count). One object
    /// bundles many partitions (FR-32), and this is each one's region of it.
    ///
    /// ⚠️ **[`ByteRange::Full`] is only correct for an object holding exactly
    /// one span.** Two spans of one bundled object that both claim the whole
    /// object are not two regions, and a reader honouring them would GET the
    /// entire bundle and decode another topic's records as its own. Nothing
    /// here can reject it — `Full` is a legal `ByteRange` — so it is `M3.13`,
    /// which writes bundled objects, that owns emitting real bounded regions,
    /// and `M3.8`'s read path that would be the victim of its not doing so.
    #[must_use]
    pub const fn bytes(&self) -> ByteRange {
        self.bytes
    }

    /// The idempotent-producer identity this span was committed under, if
    /// any.
    #[must_use]
    pub const fn producer(&self) -> Option<ProducerIdentity> {
        self.producer
    }
}

/// One durable entry in a metadata log.
///
/// # Why an event, and not a key-value pair
///
/// ⚠️ `M3.md` task 2, and the decision that is expensive to reverse. A
/// key-value record — *"partition p is now at offset n"* — is a statement of
/// **state**, and a log of those is only correct if every entry is applied
/// exactly once, in order, with nothing lost. An event record — *"this object
/// added n records to partition p"* — is a **delta**, and a log of deltas can
/// be snapshotted at any point by materializing it, because the snapshot is a
/// fold over the events rather than a copy of the last one.
///
/// `M3.md` task 2 states the consequence directly: this shape is **what makes
/// snapshotting right and compaction wrong later** (`M6`). A snapshot is a
/// fold over events and is always well defined; a compaction that rewrites
/// *state* records has to decide which one wins, and there is no answer that
/// is right for a reader mid-replay.
///
/// ⚠️ **Deliberately not `#[non_exhaustive]`**, for the reason
/// [`Error`](crate::Error) is not: a new variant here is a new event every
/// applier must be made to consider, and a `_ =>` arm absorbing it silently is
/// exactly the bug that would not surface until a replay produced the wrong
/// offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetadataRecord {
    /// An object landed in storage and now occupies a position in the log.
    ///
    /// This is the offset-assignment event: `ADR-0020` assigns at commit time,
    /// so the record is written by the coordinator once the PUT is durable,
    /// and the offsets it implies become visible only when it commits.
    BatchCommitted {
        /// The object the records are in.
        object: ObjectKey,
        /// What it added, per `(topic, partition)`. One object bundles many,
        /// which is what makes FR-32's one-PUT-across-N-topics flush possible.
        spans: Vec<CommittedSpan>,
    },
    /// A partition's history below `upto` now lives in a manifest.
    ///
    /// ⚠️ **The event that makes the index's state bounded** (`ADR-0042`). At
    /// doc 14 §3's working set a partition's history is 4M
    /// `(object, partition)` entries a second over a seven-day retention —
    /// 225.6 TB, which is not a large number but an impossible one. What the
    /// coordinator holds after this event is one reference, and the entries it
    /// replaces are in the object `manifest` names.
    ///
    /// ⚠️ **A delta, like every record here**, and that is what lets it be
    /// replayed: applying it twice is applying it once, because it names an
    /// absolute boundary rather than "drop the oldest N".
    ///
    /// ⚠️ **Compaction writes this and produce never does.** Doc 14 §7's
    /// friction 4 measures ~15 successful conditional writes/s per key on S3
    /// and ~1/s on GCS; one of these per partition per compaction round is
    /// three orders under the tighter of the two, and a produce-path writer
    /// would be three orders over it.
    ManifestPublished {
        /// The topic whose partition this is about.
        topic: TopicId,
        /// The partition.
        partition: PartitionId,
        /// The object holding the manifest.
        manifest: ObjectKey,
        /// The offset just past the last record the manifest covers.
        ///
        /// ⚠️ **Exclusive, and it must meet what is left.** The fold refuses a
        /// manifest that does not end exactly where the partition's remaining
        /// history begins: below that boundary is records nothing can serve,
        /// above it is records served twice.
        upto: Offset,
    },
    /// The log passed to a new coordinator incarnation.
    ///
    /// A reader that sees this knows the log it was following may have been
    /// rewound and that index state from the older epoch must be discarded
    /// rather than merged (`M3.md` task 3).
    EpochChanged {
        /// The incarnation now holding the log.
        epoch: CoordinatorEpoch,
    },
}
