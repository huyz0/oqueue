//! One batch's bytes, and what fetching them cost the request.
//!
//! ⚠️ **Its own module because the two tiers are asymmetric and that
//! asymmetry is the concept** (`code-structure.md` rule 18), and `read.rs`
//! reached the 500-line limit holding it once `M5.63` added a third caller. A
//! tail or manifest entry names a byte range, so its fetch is one ranged GET;
//! a history entry names none (`ADR-0022`), so its fetch is a whole object and
//! a footer resolution. What a fetch *costs* differs the same way, which is
//! why the cost travels back with the bytes.

use crate::cluster::Cluster;
use crate::read::{Fetched, Spend};
use crate::region::slice_region;
use oqueue_core::{ByteRange, IndexedBatch, PartitionId, TopicId};

impl Cluster {
    /// One batch's bytes, and what fetching them cost the request.
    ///
    /// ⚠️ **A cache hit costs nothing**, which is the whole bound: nothing
    /// dedups a client's partition list, so the objects behind it must be
    /// fetched once per request however many entries name them.
    ///
    /// ⚠️ **A history batch charges the *whole object*.** The slice handed back
    /// is one partition's region of a bundle covering every partition that
    /// flush wrote, so charging the slice would let a read pull sixty-four
    /// whole bundles off the store to answer with a megabyte — the cost
    /// `ADR-0022`'s paging rule cannot see, and the one this budget exists to
    /// bound.
    pub(super) async fn one_batch(
        &self,
        batch: &IndexedBatch,
        topic: &TopicId,
        partition: PartitionId,
        spend: &mut Spend<'_>,
    ) -> Fetched {
        match batch {
            IndexedBatch::Inline(entry) => {
                // ⚠️ **A tail read's fetch and its records are the same
                // bytes** on the success path — the range asked for *is* the
                // batch — so the caller's `max` of the two is unchanged by
                // what this reports. ⚠️ **On the failure path they are not**,
                // which is why it reports the fetch rather than nothing; the
                // comment here said "no separate fetch cost" until `M3.37`,
                // eighteen lines above the code that computes one.
                let (blob, missed) = spend
                    .objects
                    .get(self, batch.reference().object(), entry.bytes())
                    .await;
                // ⚠️ **Nothing *counted* here.** A store failure was already
                // counted by `get`, on the miss; a tail read's *parse* failure
                // happens in the caller, after the stamp, which is where it is
                // counted — and only if this was a miss.
                //
                // ⚠️ **But it is charged, and the charge is not zero.** The
                // range asked for *is* the batch, so on the success path this
                // number and the records returned are the same and the caller's
                // `max` of the two is unchanged. On the *failure* path they are
                // not: the bytes came off the store and the records did not, so
                // reporting zero would let a read that pulled a whole batch and
                // could not stamp it cost the request nothing — the tail tier's
                // version of the defect `M3.26` fixed on the history tier.
                let cost = if missed {
                    blob.as_ref().map_or(0, |bytes| bytes.len() as u64)
                } else {
                    0
                };
                Fetched { blob, cost, missed }
            }
            IndexedBatch::Footer(object) => {
                let (whole, missed) = spend
                    .objects
                    .get(self, object.object(), ByteRange::Full)
                    .await;
                match whole {
                    Ok(whole) => {
                        let cost = if missed { whole.len() as u64 } else { 0 };
                        let sliced = slice_region(&whole, topic, partition);
                        // ⚠️ **A bundle that arrived and would not parse is a
                        // failure of this request too.** The store was healthy,
                        // so `get` counted nothing — and without this a frame
                        // naming K malformed bundles escapes the failure cap
                        // entirely, which is the only bound a read that
                        // fetched-then-failed is subject to. ⚠️ **On the miss
                        // only**: the cap counts distinct objects, and a
                        // partition that took the same bad bundle out of the
                        // cache issued no GET — counting it would let one
                        // corrupt object shared by eight topics refuse six
                        // healthy partitions behind it.
                        if missed && sliced.is_err() {
                            spend.objects.note_failure();
                        }
                        Fetched {
                            blob: sliced,
                            cost,
                            missed,
                        }
                    }
                    Err(error) => Fetched {
                        blob: Err(error),
                        cost: 0,
                        missed,
                    },
                }
            }
        }
    }
}
