//! What one flush writes: many topics' records in a single object.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module. `pub(crate)` is the visibility that is actually
// true of the format details `bundle_footer` reads back — they are the two
// halves of one durable shape and nothing outside the crate touches them — so
// the lint that disagrees is the one allowed.
#![allow(clippy::redundant_pub_crate)]

use crate::{ByteRange, CommittedSpan, Error, PartitionId, ProducerIdentity, Result, TopicId};

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

/// How a region's bytes are protected.
///
/// ⚠️ **Doc 10 #40: the region header names its algorithm from its first
/// commit**, which is this one — `M1.7` found `M1` had no object format to put
/// the field on, so `roadmap.md`'s deferral table carried it here, to the first
/// commit that defines a bundled object's internal structure. A few bytes now
/// against a migration later.
///
/// ⚠️ **`M3` writes [`None`](RegionAlg::None) and reads nothing else.** `M8` is
/// where a decoder acts on another value; what matters today is that the field
/// exists, so an object written now can be told apart from one written then
/// without guessing.
///
/// ⚠️ **Per region, not per object.** `architecture.md`'s Encryption section
/// bundles topics sharing one KEK into one object, and a region's algorithm is
/// a property of the topic whose records it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RegionAlg {
    /// Stored as written. The default path, and all `M3` produces.
    None = 0,
}

impl RegionAlg {
    /// The byte a footer carries.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Reads one back.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownRegionAlg`] for any value this build does not know.
    /// ⚠️ **An error, never a default.** Treating an unknown algorithm as
    /// "stored as written" would hand a decoder ciphertext and let it decode
    /// whatever that happened to look like.
    pub const fn from_code(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::None),
            other => Err(Error::UnknownRegionAlg { code: other }),
        }
    }
}

/// One `(topic, partition)`'s region of a bundled object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub(crate) topic: TopicId,
    pub(crate) partition: PartitionId,
    pub(crate) bytes: ByteRange,
    pub(crate) record_count: u32,
    pub(crate) alg: RegionAlg,
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
        if topic.as_str().len() > MAX_TOPIC_NAME_LEN {
            return Err(Error::BundleTooLarge);
        }
        // ⚠️ **Bytes that advance no offsets are bytes nothing can ever
        // read.** The fold turns a span's count into offsets, so a region
        // carrying records under a count of zero is durable, indexed, billed
        // and unreachable — with no error anywhere. This is the mirror of the
        // empty-`records` case below, and refused for the same reason.
        if record_count == 0 {
            return Err(Error::EmptyRegion);
        }
        let offset = self.payload.len() as u64;
        let bytes = ByteRange::bounded(offset, records.len() as u64)?;
        self.payload.extend_from_slice(records);
        self.regions.push(Region {
            topic,
            partition,
            bytes,
            record_count,
            // ⚠️ `M3` writes exactly this. `M8` is where a caller chooses.
            alg: RegionAlg::None,
        });
        self.producers.push(producer);
        Ok(())
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
const MAX_TOPIC_NAME_LEN: usize = u16::MAX as usize;

fn encode_footer(regions: &[Region], out: &mut Vec<u8>) -> Result<()> {
    let start = out.len();
    for region in regions {
        let topic = region.topic.as_str().as_bytes();
        let name_len = u16::try_from(topic.len()).map_err(|_| Error::BundleTooLarge)?;
        out.extend_from_slice(&name_len.to_be_bytes());
        out.extend_from_slice(topic);
        // `PartitionId` is never negative (its own invariant), so this is a
        // widening rather than a reinterpretation.
        out.extend_from_slice(&region.partition.get().unsigned_abs().to_be_bytes());
        out.extend_from_slice(&region.record_count.to_be_bytes());
        let (offset, length) = match region.bytes {
            ByteRange::Bounded(bounded) => (bounded.offset(), bounded.length()),
            // ⚠️ **An error, not `(0, 0)`.** Unreachable from `push`, but
            // `Region` is public and this is a durable-format writer: encoding
            // a zero-length range would produce an object `parse_footer`
            // refuses, after `seal` returned `Ok`, the PUT landed and the
            // metadata record committed. Failing here costs nothing and fails
            // before anything durable exists.
            ByteRange::Full => return Err(Error::UnboundedRegion),
        };
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
        out.push(region.alg.code());
    }
    let footer_len = u32::try_from(out.len() - start).map_err(|_| Error::BundleTooLarge)?;
    let count = u32::try_from(regions.len()).map_err(|_| Error::BundleTooLarge)?;
    out.extend_from_slice(&count.to_be_bytes());
    out.push(BUNDLE_FORMAT_VERSION);
    out.extend_from_slice(&footer_len.to_be_bytes());
    Ok(())
}
