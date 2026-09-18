//! A partition's history, as an object rather than as coordinator state.
//!
//! ⚠️ **This is where the index's state goes** (`ADR-0042`). At doc 14 §3's
//! working set the coordinator cannot hold a partition's history: 4M
//! `(object, partition)` entries a second over a seven-day retention is 97 TB,
//! which is not a large number but an impossible one. What it holds instead is
//! one reference per partition, to a manifest like this — 40 MB against 97 TB.
//!
//! ⚠️ **Keyed by partition, and that is the whole decision.** A manifest
//! listing every partition of every object it names is ~1M ranges at that
//! working set; one listing a single partition's objects is ~4.6 entries after
//! compaction. Same mechanism, five orders apart. Three shapes that missed
//! this are recorded in `ADR-0041`.
//!
//! ⚠️ **Chained, because the uncompacted backlog is not small.** Between
//! compaction rounds a partition accumulates ~7,200 objects — ~169 KiB of
//! entries — so a manifest that only ever grew would be read whole on every
//! cold fetch. Past [`PARTITION_MANIFEST_BYTES`] it spills: the newest
//! manifest names its predecessor, and a reader follows the chain only for an
//! offset the newest does not cover. That is doc 14 §7's friction row 3
//! ("two-level manifest, mandatory") and Redpanda's own shape.
//!
//! ⚠️ **Compaction writes these and produce never does.** Doc 14 §7's friction
//! 4 measures ~15 successful conditional writes per second per key on S3 and
//! ~1/s on GCS; one write per partition per compaction round is 1 per 1,800 s,
//! three orders under the tighter of the two. A produce-path writer would be
//! three orders over it.

mod parse;

pub use parse::{PartitionManifest, parse_partition_manifest};

use crate::{ByteRange, Error, ObjectKey, Offset, Result};

/// The manifest format's version.
pub const PARTITION_MANIFEST_VERSION: u8 = 1;

/// What a partition manifest's trailer ends with, so bytes are
/// self-identifying.
///
/// ⚠️ **Distinct from every other format's**, for the reason `COMPOSITE_MAGIC`
/// exists: these formats all end in a count, a version and a length, so
/// without a marker one parses as another and yields keys built out of
/// somebody else's bytes.
pub const PARTITION_MANIFEST_MAGIC: [u8; 4] = *b"OQPM";

/// Trailer: `entry_count` (u32), version (u8), `body_len` (u32), magic.
pub const PARTITION_MANIFEST_TRAILER_LEN: usize = 4 + 1 + 4 + PARTITION_MANIFEST_MAGIC.len();

/// How large a manifest grows before it spills into a chain.
///
/// ⚠️ **UNDERIVED — Redpanda's measured number, not this project's.** Doc 14
/// §7's friction row 3 cites `cloud_storage_spillover_manifest_size` capping
/// their live manifest near 128 KiB; nothing here has measured anything, and
/// `ADR-0042` says so rather than dressing it up. ⚠️ **An entry here is ~36
/// bytes, not `ADR-0042`'s 24** — that figure was an estimate made before the
/// format existed, and `encode_entry` writes `2 + key + 8 + 4 + 8 + 8` — so
/// the cap is ~3,600 entries, against a compacted steady state of ~4.6 and an
/// uncompacted backlog of ~7,200 between rounds. The cap therefore bites on
/// the backlog and never on the steady state, which is the behaviour it is
/// for, and the wider entry moves it in the safe direction: sooner.
///
/// ⚠️ **Raising it is the weakening direction**: a larger cap means more bytes
/// read on every cold fetch, which is the column `ADR-0042` picked this shape
/// on. It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const PARTITION_MANIFEST_BYTES: usize = 131_072;

/// One object this partition has records in.
///
/// ⚠️ **Carries the partition's offset, which a `Region` does not.** A
/// region's byte range is inside its object; what a fetch needs first is which
/// object holds offset `o`, and that is this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntry {
    object: ObjectKey,
    base_offset: Offset,
    record_count: u32,
    bytes: ByteRange,
}

impl ManifestEntry {
    /// Names an object, where its records start, and where they are inside it.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyBundle`] if it holds no record — an entry for no records
    /// is durable, indexed and unreachable.
    /// [`Error::UnboundedRegion`] if the range is [`ByteRange::Full`]: an
    /// entry claiming the whole object would have a reader decode a
    /// neighbouring partition's records as its own.
    pub fn new(
        object: ObjectKey,
        base_offset: Offset,
        record_count: u32,
        bytes: ByteRange,
    ) -> Result<Self> {
        if record_count == 0 {
            return Err(Error::EmptyBundle);
        }
        if matches!(bytes, ByteRange::Full) {
            return Err(Error::UnboundedRegion);
        }
        Ok(Self {
            object,
            base_offset,
            record_count,
            bytes,
        })
    }

    /// The object.
    #[must_use]
    pub const fn object(&self) -> &ObjectKey {
        &self.object
    }

    /// The partition offset its first record has.
    #[must_use]
    pub const fn base_offset(&self) -> Offset {
        self.base_offset
    }

    /// How many records it holds for this partition.
    #[must_use]
    pub const fn record_count(&self) -> u32 {
        self.record_count
    }

    /// Where inside the object they are.
    #[must_use]
    pub const fn bytes(&self) -> ByteRange {
        self.bytes
    }

    /// The offset just past its last record.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`] if the fold that produced it was wrong.
    pub fn end_offset(&self) -> Result<Offset> {
        self.base_offset.add(i64::from(self.record_count))
    }
}

/// Builds a partition manifest.
///
/// ⚠️ **No `len` or `is_empty`, deliberately.** Both were here and nothing
/// called them: `seal` refuses an empty manifest, and the spill decision
/// `PARTITION_MANIFEST_BYTES` governs is about bytes rather than entries, so
/// the writer that makes it — `M5.62`'s commit path — will ask for what it
/// actually needs. An accessor with no caller is an accessor no test can
/// distinguish from a wrong one.
#[derive(Debug, Default)]
pub struct PartitionManifestBuilder {
    entries: Vec<ManifestEntry>,
    previous: Option<ObjectKey>,
}

impl PartitionManifestBuilder {
    /// An empty builder.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            previous: None,
        }
    }

    /// Names the manifest this one spills from.
    ///
    /// ⚠️ **A predecessor, not a successor.** The newest manifest is the one
    /// the coordinator points at, so the chain runs backwards in time and a
    /// reader follows it only for an offset the newest does not cover.
    #[must_use]
    pub fn spilling_from(mut self, previous: ObjectKey) -> Self {
        self.previous = Some(previous);
        self
    }

    /// Adds one object's entry.
    ///
    /// # Errors
    ///
    /// [`Error::IndexObjectMismatch`] if the object is already named, or if
    /// the entry does not begin exactly where the last one ended. ⚠️ **Both
    /// are the same hazard one step apart**: a gap is records nothing can
    /// serve and an overlap is records served twice, and a reader
    /// binary-searching this cannot tell either from a healthy manifest.
    pub fn push(&mut self, entry: ManifestEntry) -> Result<()> {
        if self
            .entries
            .iter()
            .any(|held| held.object() == entry.object())
        {
            return Err(Error::IndexObjectMismatch);
        }
        if let Some(last) = self.entries.last()
            && last.end_offset()? != entry.base_offset()
        {
            return Err(Error::IndexObjectMismatch);
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Encodes it.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyBundle`] if it holds no entry, and
    /// [`Error::BundleTooLarge`] if a key, the entry count or the body's
    /// length exceeds what the format's fields carry.
    pub fn seal(self) -> Result<Vec<u8>> {
        if self.entries.is_empty() {
            return Err(Error::EmptyBundle);
        }
        let mut out = Vec::new();
        encode_key(self.previous.as_ref(), &mut out)?;
        for entry in &self.entries {
            encode_entry(entry, &mut out)?;
        }
        let body_len = u32::try_from(out.len()).map_err(|_| Error::BundleTooLarge)?;
        let count = u32::try_from(self.entries.len()).map_err(|_| Error::BundleTooLarge)?;
        out.extend_from_slice(&count.to_be_bytes());
        out.push(PARTITION_MANIFEST_VERSION);
        out.extend_from_slice(&body_len.to_be_bytes());
        out.extend_from_slice(&PARTITION_MANIFEST_MAGIC);
        Ok(out)
    }
}

/// Writes a key, or a zero length for "no predecessor".
fn encode_key(key: Option<&ObjectKey>, out: &mut Vec<u8>) -> Result<()> {
    let raw = key.map_or("", |key| key.as_str()).as_bytes();
    let len = u16::try_from(raw.len()).map_err(|_| Error::BundleTooLarge)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(raw);
    Ok(())
}

fn encode_entry(entry: &ManifestEntry, out: &mut Vec<u8>) -> Result<()> {
    encode_key(Some(entry.object()), out)?;
    out.extend_from_slice(&entry.base_offset().get().to_be_bytes());
    out.extend_from_slice(&entry.record_count().to_be_bytes());
    let ByteRange::Bounded(bounded) = entry.bytes() else {
        // `ManifestEntry::new` refuses one, so this is unreachable through the
        // constructor -- and it is here because this writes a durable format.
        return Err(Error::UnboundedRegion);
    };
    out.extend_from_slice(&bounded.offset().to_be_bytes());
    out.extend_from_slice(&bounded.length().to_be_bytes());
    Ok(())
}
