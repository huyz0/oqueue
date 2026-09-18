//! Naming a compacted object so that no two attempts at one plan collide.
//!
//! ⚠️ **`ADR-0037`'s unique-key obligation, and it is the whole of what makes
//! the seal safe to write unconditionally.** `M5.6` dropped the `IfAbsent`
//! precondition the streaming writer used to carry — the weaker form of this
//! guarantee, which caught a reused key rather than not reusing one — and what
//! replaced it was the claim that no two attempts share a key. Until this
//! module that claim was a convention: `merge` and `merge_round` took the key
//! from their caller, and nothing minted one.
//!
//! ⚠️ **The index was already safe and the store was not**, which is why this
//! is about bytes rather than about entries. `M5.13`'s fold refuses a second
//! swap that retires references the first already retired, so the index names
//! one object whatever keys were used. The store has no such fold: two
//! attempts sharing a key means the second overwrites the bytes of the first,
//! and if the first attempt's swap committed, those are bytes an index entry
//! names and a fetch resolves.

use oqueue_core::{Error, ObjectKey, Result};

/// The prefix nothing else in this repository writes under.
///
/// ⚠️ **Distinct from `BundleNamer`'s `bundles/`, and that is load-bearing.**
/// A compacted object and a flushed one are written by different processes
/// with independently minted writer identities, so sharing a prefix would make
/// collision-freedom rest on those two identity spaces never overlapping —
/// a property nothing checks and nobody would notice failing.
const PREFIX: &str = "compacted";

/// Mints the key for each object a compaction run writes.
///
/// ⚠️ **In `BundleNamer`'s shape, and it rests on the same obligation**: a
/// monotonic sequence per writer, collision-free only while writer identities
/// are distinct. That is the composer's obligation, not this type's, and it is
/// stated here rather than assumed because it is what the unconditional seal
/// rests on — two compactors sharing an identity would overwrite each other's
/// output, and after a swap committed those are acknowledged records.
#[derive(Debug)]
pub struct CompactionNamer {
    writer: String,
    next: u64,
}

impl CompactionNamer {
    /// A namer for one compaction writer.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyObjectKey`] if `writer` is empty, and
    /// [`Error::MalformedWriterId`] if it contains a `/` — which would let two
    /// distinct identities produce one key by moving the boundary between the
    /// writer and its sequence.
    pub fn new(writer: impl Into<String>) -> Result<Self> {
        let writer = writer.into();
        if writer.is_empty() {
            return Err(Error::EmptyObjectKey);
        }
        if writer.contains('/') {
            return Err(Error::MalformedWriterId);
        }
        Ok(Self { writer, next: 0 })
    }

    /// The key for the next object this writer seals.
    ///
    /// ⚠️ Zero-padded so the sequence is fixed-width, and **not** so it sorts:
    /// S3 Express directory buckets do not return objects in lexicographic
    /// order (doc 12 §1.2), so nothing may depend on adjacent keys being
    /// adjacent under string ordering.
    ///
    /// # Errors
    ///
    /// [`Error::BundleSequenceExhausted`] if this writer has sealed
    /// `u64::MAX` objects. ⚠️ An error rather than a wrap, for the reason
    /// `Offset::add` refuses to wrap: the next name after a wrap is one
    /// already used, and the write that follows overwrites records a
    /// committed swap already named.
    pub fn next_key(&mut self) -> Result<ObjectKey> {
        let sequence = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(Error::BundleSequenceExhausted)?;
        ObjectKey::new(format!("{PREFIX}/{}/{sequence:020}", self.writer))
    }
}
