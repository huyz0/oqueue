//! What a producer is told once its object has a position.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module. `pub(crate)` is the visibility that is actually
// true of the constructors below, and why it has to be is in their doc
// comments, so the lint that disagrees is the one allowed.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{CommitVersion, CoordinatorEpoch, Offset, PartitionId, TopicId};

/// The offset a partition reports when it has none.
///
/// ⚠️ **`-1`, never `0`** (`M3.md` task 6). The protocol's own unset sentinel is
/// negative precisely because every non-negative value is a legal offset, so a
/// `0` on an error path is indistinguishable from "your records are at the
/// start of the log" — a client that trusts it re-reads someone else's records
/// or reports a lag that cannot be right. Doc 13 §8 records this happening in a
/// peer system: it turns an availability bug into a safety bug, which is a
/// change of kind rather than of degree.
pub const UNASSIGNED_OFFSET: i64 = -1;

/// Where one `(topic, partition)`'s records landed.
///
/// ⚠️ **Produced by the coordinator, never by a producer.** `ADR-0020` point 2:
/// producers race to PUT without coordinating, and the offset range an object
/// occupies is decided afterwards, when its metadata record commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    topic: TopicId,
    partition: PartitionId,
    base_offset: Offset,
    record_count: u32,
}

impl Assignment {
    /// Builds an assignment.
    ///
    /// ⚠️ **`pub(crate)`, and that is load-bearing.** An assignment is a claim
    /// that the coordinator gave these records this offset; a downstream crate
    /// able to construct one could make that claim without a coordinator
    /// having made it.
    #[must_use]
    pub(crate) const fn new(
        topic: TopicId,
        partition: PartitionId,
        base_offset: Offset,
        record_count: u32,
    ) -> Self {
        Self {
            topic,
            partition,
            base_offset,
            record_count,
        }
    }

    /// The topic the records belong to.
    #[must_use]
    pub const fn topic(&self) -> &TopicId {
        &self.topic
    }

    /// The partition the records belong to.
    #[must_use]
    pub const fn partition(&self) -> PartitionId {
        self.partition
    }

    /// The offset of the first of them.
    #[must_use]
    pub const fn base_offset(&self) -> Offset {
        self.base_offset
    }

    /// How many there are.
    #[must_use]
    pub const fn record_count(&self) -> u32 {
        self.record_count
    }
}

/// A committed position, and the epoch of the coordinator that decided it.
///
/// ⚠️ **Its existence is the durability claim.** `ADR-0020` point 3 orders the
/// coordinator assign → journal → ack, so nothing constructs one of these
/// before the metadata record covering it is durable. A caller that has one may
/// acknowledge to its client; a caller that does not has no offset to report,
/// which is what [`UNASSIGNED_OFFSET`] is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitAck {
    version: CommitVersion,
    epoch: CoordinatorEpoch,
    assignments: Vec<Assignment>,
}

impl CommitAck {
    /// Builds an ack.
    ///
    /// ⚠️ **`pub(crate)`, and that is what makes the paragraph above true.**
    /// "Its existence is the durability claim" is a sentence about a type
    /// nothing outside this crate can construct — a public constructor would
    /// let `M3.14`'s produce handler build one after a
    /// [`CoordinatorError::Journal`](crate::CoordinatorError::Journal) and
    /// acknowledge a client for records that never reached the log, which is
    /// FR-10 broken by a type whose docs say it cannot be.
    #[must_use]
    pub(crate) const fn new(
        version: CommitVersion,
        epoch: CoordinatorEpoch,
        assignments: Vec<Assignment>,
    ) -> Self {
        Self {
            version,
            epoch,
            assignments,
        }
    }

    /// The position in the metadata log this commit occupies.
    ///
    /// ⚠️ A session carries this as its read-your-writes watermark —
    /// [`ReadMode::AtLeast`](oqueue_core::ReadMode::AtLeast), hazard H2, which
    /// `M3.10` wires. It is comparable only against versions from the same
    /// metadata shard (`ADR-0020` point 1).
    #[must_use]
    pub const fn version(&self) -> CommitVersion {
        self.version
    }

    /// Which coordinator incarnation decided it.
    #[must_use]
    pub const fn epoch(&self) -> CoordinatorEpoch {
        self.epoch
    }

    /// One assignment per span the commit was given, **in the order they were
    /// given**.
    ///
    /// ⚠️ **Positional, not keyed, and that is the guarantee to attribute an
    /// offset by.** A commit may name one `(topic, partition)` in more than one
    /// span — an object bundling two producers' batches for the same partition
    /// does exactly that — so the pairs here are not distinct, and the *n*th
    /// entry describes the *n*th span and no other. A caller acknowledging the
    /// producer that contributed a particular span reads it from this slice by
    /// position; [`base_offset`](Self::base_offset) answers a different
    /// question and is not a substitute.
    #[must_use]
    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }

    /// The **first** offset this commit gave that partition, on the wire's
    /// terms.
    ///
    /// That is the whole of this commit's contribution to the partition,
    /// because the contribution is contiguous: spans naming one
    /// `(topic, partition)` are assigned consecutively, so the *k* records the
    /// commit added occupy `[base_offset, base_offset + k)` with nothing in
    /// between. It is the number a Produce response wants when the request it
    /// answers is the only thing in the commit.
    ///
    /// ⚠️ **It is not per-span attribution**, and a bundler must not use it as
    /// one. When an object carries two producers' batches for one partition,
    /// this returns where the *first* of them landed for both; the second
    /// producer's offset is its span's entry in
    /// [`assignments`](Self::assignments), read by position.
    ///
    /// [`UNASSIGNED_OFFSET`] when this ack covers no such partition — which is
    /// the same answer an error path gives, and deliberately so: "there is no
    /// offset for you" has one representation, not two.
    ///
    /// ⚠️ A linear scan, unlike the borrowed-key map the allocator's staging
    /// uses, because this walks the list once per *call* rather than once per
    /// span — and the caller that has many partitions to answer for reads
    /// [`assignments`](Self::assignments) in one pass instead.
    #[must_use]
    pub fn base_offset(&self, topic: &TopicId, partition: PartitionId) -> i64 {
        self.assignments
            .iter()
            .find(|a| a.partition() == partition && a.topic() == topic)
            .map_or(UNASSIGNED_OFFSET, |a| a.base_offset().get())
    }
}
