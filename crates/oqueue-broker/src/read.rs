//! The read path: index lookup, object reads, and the offset stamp.
//!
//! ⚠️ **Its own module because a fetch is where the index's two tiers stop
//! being symmetric.** A tail read is one GET at a range the index already
//! holds; a history read is a footer resolution first, because the index
//! deliberately does not keep byte ranges for entries past its tail window —
//! `ADR-0022`, and the ~40-bytes-per-entry arithmetic in `M3.md` is why.
//!
//! ⚠️ **A history read GETs the whole object, and that is a bound rather than
//! a choice.** Resolving a footer from a suffix needs the object's size, and
//! [`ObjectStore`](oqueue_core::ObjectStore) has no `head` — three methods,
//! `get`/`put`/`delete`. Adding one is a seam change with an ADR and every
//! implementation behind it, which this row is not. So the history path pays
//! one whole-object GET where it should pay a tail GET plus a ranged GET, and
//! `M3.22` — which owns the reader's byte budget — is where that stops.
//!
//! ⚠️ **What is *not* here is that budget and the 404 rule**, both `M3.22`'s
//! row rather than omissions. `find_batches` bounds what it can *price*, not
//! what a reader will spend, so a page of unpriceable history batches can cost
//! more than the client's `max_bytes`; and a 404 from this path means an object
//! was **reaped**, never "not yet written", so the answer is a coordinator
//! round trip and a retry rather than end-of-log. Until then this returns what
//! the index named, and surfaces a missing object as an error rather than as
//! an empty partition — the one thing doc 12 §4.6 says it must never become.

use crate::cluster::Cluster;
use oqueue_codec::batch::rewrite_base_offset;
use oqueue_core::{
    ByteRange, Error, IndexedBatch, Offset, PartitionId, Region, TopicId, parse_footer,
};

impl Cluster {
    /// Every batch this partition holds from `start`, with its real base
    /// offset stamped in.
    ///
    /// ⚠️ **A fetch *at* the high watermark issues zero GETs** (FR-12): the
    /// index names no batch at or past the end offset, so the loop below never
    /// runs and the store is never touched.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`] if a stored entry's end offset is
    /// unrepresentable; [`Error::IndexObjectMismatch`] if an object does not
    /// hold what the index said it does; and whatever the store returns for an
    /// object the index named.
    pub async fn read(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        max_bytes: u64,
    ) -> Result<Vec<u8>, Error> {
        let page = self
            .index()
            .find_batches(topic, partition, start, max_bytes)?;
        let mut records = Vec::new();
        for batch in page {
            let mut blob = match &batch {
                IndexedBatch::Inline(entry) => {
                    self.store()
                        .get(batch.reference().object(), entry.bytes())
                        .await?
                }
                IndexedBatch::Footer(object) => {
                    let whole = self.store().get(object.object(), ByteRange::Full).await?;
                    slice_region(&whole, topic, partition)?
                }
            };
            // ⚠️ The stamp `cluster.rs`'s module doc explains: bytes in storage
            // carry whatever base offset the producer sent, because they were
            // written before anyone knew what order they landed in. A blob too
            // short to hold a batch header is an object disagreeing with the
            // index that named it, so the read fails rather than serving it.
            rewrite_base_offset(&mut blob, batch.reference().base_offset().get(), 0)
                .map_err(|_| Error::IndexObjectMismatch)?;
            records.append(&mut blob);
        }
        Ok(records)
    }
}

/// This partition's records, cut out of a whole bundled object.
fn slice_region(whole: &[u8], topic: &TopicId, partition: PartitionId) -> Result<Vec<u8>, Error> {
    let size = whole.len() as u64;
    let regions = parse_footer(whole, size)?;
    let ByteRange::Bounded(bounds) = region_for(&regions, topic, partition)? else {
        // `parse_footer` refuses an unbounded region, so this arm is
        // unreachable — named rather than `unwrap`ped, because a panic here
        // would be reachable from stored bytes (`security.md` rule 3).
        return Err(Error::IndexObjectMismatch);
    };
    let from = usize::try_from(bounds.offset()).map_err(|_| Error::IndexObjectMismatch)?;
    let len = usize::try_from(bounds.length()).map_err(|_| Error::IndexObjectMismatch)?;
    let end = from.checked_add(len).ok_or(Error::IndexObjectMismatch)?;
    whole
        .get(from..end)
        .map(<[u8]>::to_vec)
        .ok_or(Error::IndexObjectMismatch)
}

/// The one region in `regions` this partition's records live in.
///
/// ⚠️ **Exactly one, and a second is an error rather than a choice.** Regions
/// carry no offsets — offsets are assigned at commit, after the object is
/// written — so two regions for one `(topic, partition)` in one object leave
/// nothing to say which of them an `ObjectRef` names. The write path is what
/// keeps that from happening: one `Produce` request carries at most one batch
/// per partition, so one flush pushes at most one region for it. This is where
/// that assumption is checked rather than trusted.
fn region_for(
    regions: &[Region],
    topic: &TopicId,
    partition: PartitionId,
) -> Result<ByteRange, Error> {
    let mut found = regions
        .iter()
        .filter(|region| region.topic() == topic && region.partition() == partition);
    let region = found.next().ok_or(Error::IndexObjectMismatch)?;
    if found.next().is_some() {
        return Err(Error::IndexObjectMismatch);
    }
    Ok(region.bytes())
}
