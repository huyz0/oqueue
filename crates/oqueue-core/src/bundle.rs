//! What one flush writes: many topics' records in a single object.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module. `pub(crate)` is the visibility that is actually
// true of the format details `bundle_footer` reads back — they are the two
// halves of one durable shape and nothing outside the crate touches them — so
// the lint that disagrees is the one allowed.
#![allow(clippy::redundant_pub_crate)]

mod alg;
mod encode;

pub use alg::RegionAlg;

use crate::{
    ByteRange, CommittedSpan, Error, PartitionId, ProducerIdentity, RegionEnvelope, Result,
    SealedRegion, TopicId,
};
use encode::encode_footer;

/// The format version this crate writes, and the only one it reads.
///
/// ⚠️ **In the trailer at the object's tail, so a reader knows before it
/// parses the regions** — byte four of the nine, after the region count and
/// before the footer's length, in a nine-byte trailer. An object outlives the
/// process that wrote it — `M5` rewrites objects `M3`
/// produced, and `M6` reads a log across a restart — so the shape has to be
/// self-describing from the first commit rather than from the first time it
/// changes.
pub const BUNDLE_FORMAT_VERSION: u8 = 1;

/// One `(topic, partition)`'s region of a bundled object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub(crate) topic: TopicId,
    pub(crate) partition: PartitionId,
    pub(crate) bytes: ByteRange,
    pub(crate) record_count: u32,
    pub(crate) alg: RegionAlg,
    /// `Some` exactly when [`alg`](Self::alg) is not
    /// [`RegionAlg::None`](RegionAlg::None) — the two fields are one fact, and
    /// `bundle/encode.rs` refuses either contradiction rather than writing an
    /// object no reader can make sense of.
    pub(crate) envelope: Option<RegionEnvelope>,
}

impl Region {
    /// Where this partition's records live inside the object.
    ///
    /// ⚠️ Always bounded, never [`ByteRange::Full`] — an object holding N
    /// regions has none that is the whole of it, and two regions both claiming
    /// the whole object would have a reader decode another topic's records as
    /// its own. [`BundleBuilder`] is what makes that unrepresentable.
    #[must_use]
    pub const fn bytes(&self) -> ByteRange {
        self.bytes
    }

    /// The topic whose records these are.
    #[must_use]
    pub const fn topic(&self) -> &TopicId {
        &self.topic
    }

    /// The partition whose records these are.
    #[must_use]
    pub const fn partition(&self) -> PartitionId {
        self.partition
    }

    /// How many records the region holds.
    #[must_use]
    pub const fn record_count(&self) -> u32 {
        self.record_count
    }

    /// How the region's bytes are protected.
    #[must_use]
    pub const fn alg(&self) -> RegionAlg {
        self.alg
    }

    /// The key id, wrapped DEK and nonce a sealed region carries, and `None`
    /// for a region stored as written.
    ///
    /// ⚠️ **`None` here and [`RegionAlg::None`] are the same statement**, and
    /// a reader must not take either one as permission to serve the bytes: a
    /// header byte is not authenticated, so what a region *must* be is derived
    /// from its topic's key domain rather than from what the footer says it is
    /// (`M8.12`).
    #[must_use]
    pub const fn envelope(&self) -> Option<&RegionEnvelope> {
        self.envelope.as_ref()
    }
}

/// Accumulates regions into the single object one flush writes.
///
/// # ⚠️ One flush is one PUT, and that is the whole cost model
///
/// FR-32, and `M3.md` task 19: *"one flush covering N topics issues **one
/// PUT**… This is what the whole cost model rests on."* Doc 12 prices a PUT far
/// above the bytes in it, so a broker that wrote one object per topic would pay
/// per topic for a workload whose cost is supposed to scale with bytes. This
/// type is what makes "one flush, one object" the only expressible shape: it
/// hands back one payload, and there is no way to ask it for two.
///
/// ⚠️ **A flush must not span metadata shards** (`ADR-0020`): an object bundles
/// topics within one shard, so committing it stays a single append to one log
/// rather than a non-atomic append to two — a crash between the two appends
/// leaves the object committed for some partitions and not others.
///
/// ⚠️ **Nothing here checks it, and `M3.13`'s row asked for it to be
/// enforced.** `MetadataShardId` is `M7`'s, so [`push`](Self::push) has nothing
/// to compare a topic's shard against. That is a **deferral rather than a
/// note**: `roadmap.md`'s table carries it and `M7.md` task 17b receives it,
/// beside the shard identity it needs, so the rule lands *with* the sharding
/// rather than being rediscovered by whoever first writes a bundle across two.
#[derive(Debug, Default)]
pub struct BundleBuilder {
    payload: Vec<u8>,
    regions: Vec<Region>,
    // ⚠️ **Parallel to `regions`, not a field on it.** `Region` is the
    // object's own on-disk footer shape (`encode_footer`/`parse_footer`
    // round-trip it byte for byte); a producer's identity is `ADR-0031`'s
    // metadata-log concern, not the object format's, so it rides beside
    // `regions` — pushed in lockstep by `push` — rather than widening what a
    // reader parses back out of the object's own bytes. `seal` zips the two
    // to build each `CommittedSpan`.
    producers: Vec<Option<ProducerIdentity>>,
}

/// What one produce contributes to the span [`BundleBuilder::push`] will
/// build: how many records, and — for an idempotent producer — whose
/// sequence they extend.
///
/// ⚠️ **Grouped so `push` stays at `clippy.toml`'s five-argument threshold**,
/// the same reason [`ProducerIdentity`] itself groups three fields
/// (`producer_identity.rs`) — not an invented split: both fields here are
/// exactly what [`CommittedSpan::new`] needs beyond a region's topic,
/// partition and byte range, so this is "the `CommittedSpan`-bound facts one
/// push supplies" as against `push`'s other two arguments, which are the
/// region's own addressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushedRecords {
    /// The record count, floored at one — the offsets this batch will
    /// occupy.
    pub count: u32,
    /// `None` for an ordinary produce; `Some` for one `M11`'s allocator can
    /// deduplicate.
    pub producer: Option<ProducerIdentity>,
}

impl BundleBuilder {
    /// An empty bundle.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            payload: Vec::new(),
            regions: Vec::new(),
            producers: Vec::new(),
        }
    }

    /// Appends one `(topic, partition)`'s records.
    ///
    /// ⚠️ **The region's range is computed here and cannot be supplied**, which
    /// is what makes the ranges disjoint and in order by construction rather
    /// than by a caller being careful.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyByteRange`] if `records` is empty. A region nothing can be
    /// read from is a region whose `ByteRange` cannot be built, and admitting
    /// one would put a zero-length range in a footer for a reader to trip over.
    ///
    /// [`Error::EmptyRegion`] if `pushed.count` is zero. Bytes that advance no
    /// offsets are bytes nothing can ever read.
    ///
    /// [`Error::BundleTooLarge`] if the topic name will not fit the footer's
    /// length field — checked here, before any bytes are copied, rather than at
    /// `seal` when the payload is already built.
    pub fn push(
        &mut self,
        topic: TopicId,
        partition: PartitionId,
        pushed: PushedRecords,
        records: &[u8],
    ) -> Result<()> {
        let PushedRecords {
            count: record_count,
            producer,
        } = pushed;
        admissible(&topic, record_count)?;
        let offset = self.payload.len() as u64;
        let bytes = ByteRange::bounded(offset, records.len() as u64)?;
        self.payload.extend_from_slice(records);
        self.regions.push(Region {
            topic,
            partition,
            bytes,
            record_count,
            // ⚠️ `M3` writes exactly this; `M8.4`'s `push_sealed` is where a
            // caller chooses otherwise, and this path's bytes are unchanged by
            // its arrival — an unsealed region encodes exactly what it always
            // did.
            alg: RegionAlg::None,
            envelope: None,
        });
        self.producers.push(producer);
        Ok(())
    }

    /// Appends one `(topic, partition)`'s **sealed** records, with the
    /// envelope that opens them.
    ///
    /// ⚠️ **The bytes are sealed before they arrive** (`ADR-0050` point 1):
    /// the caller minted a [`Nonce`](crate::Nonce), spent it on
    /// `oqueue-crypto::seal`, and passes the ciphertext ‖ tag here with the
    /// envelope recording what a reader needs to undo it. The region's byte
    /// range therefore covers the tag, which is what a reader must GET.
    ///
    /// # Errors
    ///
    /// As [`BundleBuilder::push`], plus [`Error::RegionNotEncrypted`] if
    /// `sealed.alg` is [`RegionAlg::None`] — an envelope over bytes stored as
    /// written is a caller that has not decided whether this region is
    /// encrypted, and the footer has no way to record the contradiction.
    pub fn push_sealed(
        &mut self,
        topic: TopicId,
        partition: PartitionId,
        pushed: PushedRecords,
        sealed: SealedRegion<'_>,
    ) -> Result<()> {
        let PushedRecords {
            count: record_count,
            producer,
        } = pushed;
        admissible(&topic, record_count)?;
        if sealed.alg == RegionAlg::None {
            return Err(Error::RegionNotEncrypted);
        }
        let offset = self.payload.len() as u64;
        let bytes = ByteRange::bounded(offset, sealed.bytes.len() as u64)?;
        self.payload.extend_from_slice(sealed.bytes);
        self.regions.push(Region {
            topic,
            partition,
            bytes,
            record_count,
            alg: sealed.alg,
            envelope: Some(sealed.envelope),
        });
        self.producers.push(producer);
        Ok(())
    }

    /// Records a region whose bytes go somewhere else.
    ///
    /// ⚠️ **For [`BundleStream`](crate::BundleStream) only**, which streams the
    /// payload to an object store as it fills and needs the footer's regions
    /// built by the same encoder a `put` would use. `offset` is where the
    /// region's bytes start in the object being streamed, and `length` how many
    /// there are — the two this type otherwise computes from its own payload.
    ///
    /// # Errors
    ///
    /// As [`BundleBuilder::push`].
    pub(crate) fn push_streamed(
        &mut self,
        topic: TopicId,
        partition: PartitionId,
        pushed: PushedRecords,
        at: (u64, usize),
    ) -> Result<()> {
        let (offset, length) = at;
        let PushedRecords {
            count: record_count,
            producer,
        } = pushed;
        // ⚠️ **One statement of the two rules, shared with `push`.** A second
        // copy of the topic-name bound and the empty-region refusal would be
        // two statements of one rule, and the one nobody exercises is the one
        // that drifts.
        admissible(&topic, record_count)?;
        let bytes = ByteRange::bounded(offset, length as u64)?;
        self.regions.push(Region {
            topic,
            partition,
            bytes,
            record_count,
            alg: RegionAlg::None,
            envelope: None,
        });
        self.producers.push(producer);
        Ok(())
    }

    /// The encoded footer and the spans, for a payload written elsewhere.
    ///
    /// # Errors
    ///
    /// As [`BundleBuilder::seal`].
    pub(crate) fn into_footer(self) -> Result<(Vec<u8>, Vec<CommittedSpan>)> {
        if self.regions.is_empty() {
            return Err(Error::EmptyBundle);
        }
        let spans = self
            .regions
            .iter()
            .zip(&self.producers)
            .map(|(region, producer)| {
                CommittedSpan::new(
                    region.topic.clone(),
                    region.partition,
                    region.record_count,
                    region.bytes,
                    *producer,
                )
            })
            .collect();
        let mut footer = Vec::new();
        encode_footer(&self.regions, &mut footer)?;
        Ok((footer, spans))
    }

    /// How many regions have been added.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.regions.len()
    }

    /// Whether nothing has been added.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Seals the bundle: the bytes to PUT, and the spans to commit.
    ///
    /// ⚠️ **Both together, from one call.** The spans describe the object the
    /// bytes are, so producing them separately would be two derivations of one
    /// fact and a chance for the footer and the metadata record to disagree
    /// about where a topic's records are.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyBundle`] if nothing was pushed. An empty object is a PUT
    /// that costs what a full one costs and carries nothing, and a commit of no
    /// spans burns a `CommitVersion` for a record that says nothing happened.
    ///
    /// [`Error::BundleTooLarge`] if the footer would not fit its own length
    /// fields — unreachable in practice at `u32`, and an error rather than a
    /// truncated length, which would describe fewer regions than the object
    /// holds and have a reader take another topic's bytes as its own.
    pub fn seal(self) -> Result<Sealed> {
        if self.regions.is_empty() {
            return Err(Error::EmptyBundle);
        }
        let spans = self
            .regions
            .iter()
            .zip(&self.producers)
            .map(|(region, producer)| {
                CommittedSpan::new(
                    region.topic.clone(),
                    region.partition,
                    region.record_count,
                    region.bytes,
                    *producer,
                )
            })
            .collect();
        let mut payload = self.payload;
        encode_footer(&self.regions, &mut payload)?;
        Ok(Sealed {
            payload,
            regions: self.regions,
            spans,
        })
    }
}

/// The two refusals every region must pass, wherever it was pushed from.
///
/// # Errors
///
/// [`Error::BundleTooLarge`] if the topic name will not fit the footer's length
/// field — checked before any bytes are copied rather than at `seal`, when the
/// payload is already built.
///
/// [`Error::EmptyRegion`] if `record_count` is zero. ⚠️ **Bytes that advance no
/// offsets are bytes nothing can ever read**: the fold turns a span's count
/// into offsets, so a region carrying records under a count of zero is durable,
/// indexed, billed and unreachable — with no error anywhere.
fn admissible(topic: &TopicId, record_count: u32) -> Result<()> {
    if topic.as_str().len() > MAX_TOPIC_NAME_LEN {
        return Err(Error::BundleTooLarge);
    }
    if record_count == 0 {
        return Err(Error::EmptyRegion);
    }
    Ok(())
}

/// A sealed bundle: one payload, and the spans that describe it.
#[derive(Debug)]
pub struct Sealed {
    payload: Vec<u8>,
    regions: Vec<Region>,
    spans: Vec<CommittedSpan>,
}

impl Sealed {
    /// The bytes of the single object this flush writes.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// The bytes, taken — what goes to
    /// [`ObjectStore::put`](crate::ObjectStore::put).
    #[must_use]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    /// The regions, in the order they were written.
    #[must_use]
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// What the coordinator commits for this object.
    #[must_use]
    pub fn spans(&self) -> &[CommittedSpan] {
        &self.spans
    }
}

/// The trailer's fixed tail: the region count, the version, and the footer's
/// own length.
///
/// ⚠️ **Length last, so a reader can find the footer from the end.** A reader
/// that has the object's size can GET the final bytes, read the length, and
/// then GET exactly the footer — doc 12 §6.3's 1–3 GETs. A length written
/// first would need the whole object read to reach it.
///
/// ⚠️ **Both counts are `u32`, not `u16`, and that is a correctness choice
/// rather than a generous one.** FR-32 bundles as many topics as one flush
/// touches, and a `u16` region count silently truncating at 65,536 would write
/// a footer describing fewer regions than the object holds — a reader would
/// then take another topic's bytes as its own. `try_from` guards the
/// conversion anyway, so the limit is an error rather than a wrap; the wider
/// field is what keeps that error unreachable in practice.
pub(crate) const TRAILER_LEN: usize = 9;

/// The longest topic name a footer can carry.
///
/// ⚠️ A bound the *format* imposes, checked where the region is added rather
/// than where it is written, so an over-long name is refused before any bytes
/// are copied. Kafka's own limit is 249 characters, so nothing this project
/// accepts on the wire comes close.
pub(crate) const MAX_TOPIC_NAME_LEN: usize = u16::MAX as usize;
