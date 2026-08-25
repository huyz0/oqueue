//! The index's storage layout: what a fetch resolves an offset through.

use crate::{ByteRange, ObjectKey, Offset, Result};

/// One object's contribution to one partition, as the *history* tier holds it.
///
/// # ⚠️ Size is a design constraint here, not an afterthought
///
/// `M3.md` task 7 budgets ~40 bytes per entry and the size test pins the
/// inline half of it.
///
/// ⚠️ **But per-entry size is not what doc 14 §3's four orders of magnitude
/// are about, and this index does not yet reach the cheap row.** That table's
/// two rows differ by *granularity*, not field width: per-(object, partition)
/// entries arrive at ~4M/s and grow the index at ~160 MB/s, while **per-object
/// only** entries arrive at ~400/s and grow it at ~16 KB/s, with
/// partition→range left to the object's own footer.
///
/// `IndexState` stores one entry **per (object, partition) span**, which is
/// the ~4M/s row. Dropping the [`ByteRange`] on demotion cuts what each entry
/// costs; it does not cut how many there are. The coarse per-object index doc
/// 15 §7 resolves doc 10 #8 to is therefore **not built here** — this is the
/// two-tier *shape*, with the cheap tier still at the expensive granularity.
/// Closing that is a change to how entries are keyed, not to this struct, and
/// it is what `M3.11`'s quota will otherwise spend its time tripping over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    object: ObjectKey,
    base_offset: Offset,
    record_count: u32,
}

impl ObjectRef {
    /// Builds a reference.
    #[must_use]
    pub const fn new(object: ObjectKey, base_offset: Offset, record_count: u32) -> Self {
        Self {
            object,
            base_offset,
            record_count,
        }
    }

    /// The object holding the records.
    #[must_use]
    pub const fn object(&self) -> &ObjectKey {
        &self.object
    }

    /// The offset of the first record this object contributed.
    #[must_use]
    pub const fn base_offset(&self) -> Offset {
        self.base_offset
    }

    /// How many records it contributed.
    #[must_use]
    pub const fn record_count(&self) -> u32 {
        self.record_count
    }

    /// The offset just past the last record this object contributed.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`](crate::Error::OffsetOverflow) if the sum
    /// would leave the protocol's `i64` range.
    pub const fn end_offset(&self) -> Result<Offset> {
        self.base_offset.add(self.record_count as i64)
    }

    /// Whether this object holds `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`](crate::Error::OffsetOverflow) if this entry's
    /// end offset is unrepresentable.
    pub fn contains(&self, offset: Offset) -> Result<bool> {
        Ok(offset >= self.base_offset && offset < self.end_offset()?)
    }
}

/// A *tail* tier entry: the same reference, plus the byte range inline.
///
/// ⚠️ **The inline [`ByteRange`] is what makes a tail read one GET**, and it is
/// the more expensive of the two tiers per entry. The window is bounded so
/// that cost is bounded per partition; entries leaving it demote to a bare
/// [`ObjectRef`], whose range a reader resolves from the object's own footer
/// at 1–3 GETs (doc 12 §6.3). ⚠️ Bounding the window bounds per-entry cost
/// per partition and **not** the number of entries — see [`ObjectRef`] for
/// why that leaves this index on doc 14 §3's expensive row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailEntry {
    reference: ObjectRef,
    bytes: ByteRange,
}

impl TailEntry {
    /// Builds a tail entry.
    #[must_use]
    pub const fn new(reference: ObjectRef, bytes: ByteRange) -> Self {
        Self { reference, bytes }
    }

    /// The underlying reference.
    #[must_use]
    pub const fn reference(&self) -> &ObjectRef {
        &self.reference
    }

    /// Where inside the object this partition's records live.
    #[must_use]
    pub const fn bytes(&self) -> ByteRange {
        self.bytes
    }

    /// Drops the inline range, demoting this entry to the history tier.
    #[must_use]
    pub fn demote(self) -> ObjectRef {
        self.reference
    }
}
