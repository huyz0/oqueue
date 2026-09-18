//! Resolving a partition manifest into the batches a fetch reads.
//!
//! ⚠️ **`ADR-0042`'s read column, and the reason this is not in the index.**
//! A manifest is an object; [`MaterializedIndex`](oqueue_core::MaterializedIndex)
//! is synchronous by contract because every implementation of it is a local
//! fold. So the index says *where* a partition's older history is named and
//! this reads it — one GET for the manifest, a binary search, and the entries
//! it hands back are ordinary ranged reads.
//!
//! ⚠️ **The read count is the claim.** Cold, a history fetch is two GETs: the
//! manifest and the object. Once the manifest is in the request's cache it is
//! one. Across a chain hop — an offset older than the manifest the coordinator
//! points at — it is three, because the chain runs backwards in time and the
//! reader follows `previous` only when the search misses below.

use crate::cluster::Cluster;
use crate::read::Spend;
use oqueue_core::{
    ByteRange, Error, IndexedBatch, MAX_BATCHES_PER_PAGE, ObjectKey, ObjectRef, Offset, TailEntry,
    parse_partition_manifest,
};

/// How many links of a manifest chain one lookup follows before giving up.
///
/// ⚠️ **A bound, because a chain is data and data can be wrong.** A manifest
/// naming itself, or a cycle, is a read that never returns — and NFR-30's
/// "bounded GETs" is a property of the reader, not a hope about the writer.
/// Sixteen links at `PARTITION_MANIFEST_BYTES` is far more history than the
/// retention this project targets keeps.
const MAX_MANIFEST_HOPS: usize = 16;

impl Cluster {
    /// The batches a fetch from `start` reads out of `manifest`, if it names
    /// them.
    ///
    /// Empty when the offset is older than every link of the chain — the
    /// reaped case, which the caller answers as it answers an object the store
    /// no longer has — and empty when it is newer than the manifest's last
    /// entry, which is the ordinary case of a fetch the index itself names.
    ///
    /// ⚠️ **It returns ranged entries, not footer ones.** A manifest entry
    /// already carries the partition's byte range inside its object, so this
    /// tier costs one ranged GET where the un-compacted history tier costs a
    /// whole-object read to resolve a footer (`ADR-0022`). That is the read
    /// column `ADR-0042` priced.
    ///
    /// # Errors
    ///
    /// Whatever the store returns for a manifest object, and
    /// [`Error::IndexObjectMismatch`] for a manifest that does not parse or a
    /// chain longer than `MAX_MANIFEST_HOPS`.
    pub(super) async fn manifest_batches(
        &self,
        manifest: &ObjectKey,
        start: Offset,
        upto: Offset,
        spend: &mut Spend<'_>,
    ) -> Resolved {
        let mut key = manifest.clone();
        let mut cost = 0;
        for _ in 0..MAX_MANIFEST_HOPS {
            let (bytes, missed) = spend.objects.get(self, &key, ByteRange::Full).await;
            // ⚠️ **Charged on the miss, like every other GET this request
            // makes.** A manifest is up to `PARTITION_MANIFEST_BYTES`, one per
            // partition a frame names and one more per chain link followed, so
            // a tier that reported no cost would let a client with a
            // `max_bytes` of one pull megabytes off the store and leave the
            // budget unable to cut the partitions behind it off — the defect
            // `M3.26` fixed on the history tier and `M3.37` on the tail's.
            let bytes = match bytes {
                Ok(bytes) => {
                    if missed {
                        cost += bytes.len() as u64;
                    }
                    bytes
                }
                Err(error) => return Resolved::failed(error, cost),
            };
            let parsed = match parse_partition_manifest(&bytes) {
                Ok(parsed) => parsed,
                Err(error) => return Resolved::failed(error, cost),
            };
            if let Some(from) = parsed.position(start) {
                // ⚠️ **Nothing at or above `upto`, whatever the object holds.**
                // The index names everything from `upto` up, so an entry above
                // it would be served by both tiers and the fetch would return
                // those records twice — which no reader can tell from a
                // partition genuinely holding them twice. A manifest reaching
                // past the offset it was published at is what a compaction
                // produces whenever the tail advances between building it and
                // publishing it, so this is the reader's own check rather than
                // an assumption about the writer.
                return Resolved::found(
                    parsed.entries()[from..]
                        .iter()
                        .take_while(|entry| entry.base_offset() < upto)
                        .take(MAX_BATCHES_PER_PAGE)
                        .map(|entry| {
                            IndexedBatch::Inline(TailEntry::new(
                                ObjectRef::new(
                                    entry.object().clone(),
                                    entry.base_offset(),
                                    entry.record_count(),
                                ),
                                entry.bytes(),
                            ))
                        })
                        .collect(),
                    cost,
                );
            }
            // ⚠️ **Older than this link, not newer.** The search misses both
            // below the manifest's first entry and above its last; the chain
            // runs backwards, so following it can only help the first. Above
            // the last is not a miss at all — `find_batches` names those — and
            // following the chain for it would spend a GET per link to find
            // nothing.
            let older = parsed
                .entries()
                .first()
                .is_some_and(|first| start < first.base_offset());
            let Some(previous) = parsed.previous().filter(|_| older) else {
                return Resolved::found(Vec::new(), cost);
            };
            key = previous.clone();
        }
        // A chain longer than the bound is a chain this reader will not
        // follow, and saying so beats looping.
        Resolved::failed(Error::IndexObjectMismatch, cost)
    }
}

/// What resolving a manifest produced, and what it cost.
///
/// ⚠️ **The cost travels with the outcome, including the failing one.** A
/// resolution that pulled two chain links and then found a manifest it could
/// not parse spent exactly what a successful one would have, and a failure
/// reporting zero is how a frame naming K bad manifests buys K whole-object
/// GETs charged to nothing.
pub(super) struct Resolved {
    pub(super) batches: Result<Vec<IndexedBatch>, Error>,
    pub(super) cost: u64,
}

impl Resolved {
    const fn found(batches: Vec<IndexedBatch>, cost: u64) -> Self {
        Self {
            batches: Ok(batches),
            cost,
        }
    }

    const fn failed(error: Error, cost: u64) -> Self {
        Self {
            batches: Err(error),
            cost,
        }
    }
}
