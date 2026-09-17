//! Ranged-`get` request merging: several byte ranges of one object, answered
//! by fewer backend fetches than one-per-range, below a configured sparsity
//! threshold.
//!
//! ⚠️ **Narrowed before implementation, spec-check step of picking this task
//! up.** `backlog.md`'s `M1.19` row, read literally, describes coalescing
//! ranged-`get` calls that arrive independently over time from concurrent
//! callers — "calls in flight are merged." That needs either a real clock (a
//! debounce window to let concurrent arrivals accumulate before dispatching)
//! or an explicit multi-caller synchronization barrier; the former is a
//! sans-io violation this crate cannot make (NFR-51), and the latter has no
//! natural trigger when callers arrive independently and asynchronously.
//! What lands instead: a caller submits a **batch** of ranges it already
//! knows it wants together — the shape a Fetch response actually has once
//! `M2` exists (several partitions' next reads, several ranges, known before
//! any of them is dispatched) — and [`MergingObjectStore::get_many`] merges
//! *that* batch's ranges by gap, purely and synchronously, before issuing any
//! backend call at all. No clock, no cross-task coordination, no new
//! dependency: the same reason [`crate::CountingObjectStore`] lives here
//! rather than in `oqueue-store`.

use crate::{
    BoxFuture, ByteRange, Error, ObjectKey, ObjectMeta, ObjectStore, Precondition, Result,
};

/// One merged backend fetch spanning `[start, end)`, and which of the
/// caller's original ranges (by index into its input slice) it answers.
///
/// `members`' second and third fields are the member's own slice *local to
/// this group* — `local_start..local_end` into the group's eventually
/// fetched bytes — computed once, so [`MergingObjectStore::get_many`] never
/// repeats the subtraction.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MergedGroup {
    start: u64,
    end: u64,
    members: Vec<(usize, u64, u64)>,
}

/// Groups `spans` — `(original_index, start, end)` triples, one per
/// [`ByteRange::Bounded`] range in a caller's batch — into merged fetch
/// spans.
///
/// ⚠️ **Pulled out on its own**, same reasoning as every other pure helper
/// in this workspace: this is the entire size/threshold decision, with no
/// network call anywhere in it, so it is what a T0 test can reach directly.
///
/// Greedy by ascending `start`: consecutive spans whose gap (`next.start -
/// current.end`) is at most `sparsity_threshold` join the same group, and a
/// group's `end` only ever grows to cover every member's own `end` — a span
/// fully nested inside an earlier, wider one still joins correctly since the
/// gap is negative (`next.start <= current.end`). `sparsity_threshold` is a
/// ceiling: a gap larger than it keeps the two spans in separate groups
/// rather than fetching the (sparse, mostly-wasted) bytes between them.
fn plan_merge(spans: &[(usize, u64, u64)], sparsity_threshold: u64) -> Vec<MergedGroup> {
    let mut spans = spans.to_vec();
    spans.sort_unstable_by_key(|&(_, start, _)| start);

    let mut groups: Vec<MergedGroup> = Vec::new();
    for (idx, start, end) in spans {
        if let Some(last) = groups.last_mut()
            && start <= last.end.saturating_add(sparsity_threshold)
        {
            last.end = last.end.max(end);
            last.members.push((idx, start, end));
            continue;
        }
        groups.push(MergedGroup {
            start,
            end,
            members: vec![(idx, start, end)],
        });
    }

    // Members were pushed as absolute (start, end); rewrite each to its own
    // slice local to the group's eventual fetch, once, here — not on every
    // read of it.
    for group in &mut groups {
        for member in &mut group.members {
            member.1 -= group.start;
            member.2 -= group.start;
        }
    }
    groups
}

/// Wraps any [`ObjectStore`], adding [`MergingObjectStore::get_many`] —
/// several ranged reads of one object, answered by fewer backend fetches.
///
/// Implements [`ObjectStore`] itself, forwarding `get`/`put`/`delete`
/// unchanged, so it composes with [`crate::CountingObjectStore`] (in either
/// order) exactly like any other decorator over the seam.
#[derive(Debug)]
pub struct MergingObjectStore<S> {
    inner: S,
    /// The largest gap, in bytes, between two ranges that still merges them
    /// into one fetch. `0` merges only overlapping or exactly-adjacent
    /// ranges.
    sparsity_threshold: u64,
}

impl<S> MergingObjectStore<S> {
    /// Wraps `inner`, merging batches presented to [`MergingObjectStore::get_many`]
    /// whose gaps are at most `sparsity_threshold` bytes.
    #[must_use]
    pub const fn new(inner: S, sparsity_threshold: u64) -> Self {
        Self {
            inner,
            sparsity_threshold,
        }
    }
}

impl<S: ObjectStore> MergingObjectStore<S> {
    /// Fetches every range in `ranges`, in the same order, merging
    /// [`ByteRange::Bounded`] entries whose gaps are within this store's
    /// `sparsity_threshold` into a single backend call apiece.
    /// [`ByteRange::Full`] entries are never merged with anything — a whole
    /// object's size is not known ahead of the fetch that would report it,
    /// so there is no numeric span to merge by — and always cost their own
    /// backend call.
    ///
    /// Every element of the returned `Vec` corresponds to the
    /// same-index element of `ranges`; a range whose merged group's fetch
    /// failed reports that same failure, independently, for every member
    /// of the group — the reason [`Error`] gained `Clone` (this module's own
    /// doc comment).
    pub async fn get_many(&self, key: &ObjectKey, ranges: &[ByteRange]) -> Vec<Result<Vec<u8>>> {
        let mut results: Vec<Option<Result<Vec<u8>>>> = ranges.iter().map(|_| None).collect();
        let mut spans = Vec::new();
        for (index, range) in ranges.iter().enumerate() {
            match range {
                ByteRange::Full => {
                    results[index] = Some(self.inner.get(key, ByteRange::Full).await);
                }
                ByteRange::Bounded(bounded) => {
                    let start = bounded.offset();
                    match start.checked_add(bounded.length()) {
                        Some(end) => spans.push((index, start, end)),
                        // An address no real object could ever have — same
                        // reasoning `s3.rs`'s `requested_range` gives for the
                        // single-range case this generalizes.
                        None => {
                            results[index] = Some(Err(Error::ByteRangeOutOfBounds {
                                key: key.clone(),
                                offset: bounded.offset(),
                                length: bounded.length(),
                                object_size: u64::MAX,
                            }));
                        }
                    }
                }
            }
        }

        for group in plan_merge(&spans, self.sparsity_threshold) {
            // Every span came from a `Bounded` range, whose own constructor
            // (`ByteRange::bounded`) already refuses a zero length — so
            // `group.end > group.start` always, and this never fails.
            let length = group.end - group.start;
            let Ok(range) = ByteRange::bounded(group.start, length) else {
                unreachable!("a merged group's length is always non-zero");
            };
            let fetch = self.inner.get(key, range).await;
            for (index, local_start, local_end) in group.members {
                let start = usize::try_from(local_start).unwrap_or(usize::MAX);
                let end = usize::try_from(local_end).unwrap_or(usize::MAX);
                results[index] = Some(match &fetch {
                    Ok(bytes) => Ok(bytes[start..end].to_vec()),
                    Err(error) => Err(error.clone()),
                });
            }
        }

        results
            .into_iter()
            .map(|slot| slot.unwrap_or(Err(Error::Permanent)))
            .collect()
    }
}

impl<S: ObjectStore> ObjectStore for MergingObjectStore<S> {
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>> {
        self.inner.get(key, range)
    }

    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<Precondition>,
    ) -> BoxFuture<'a, Result<ObjectMeta>> {
        self.inner.put(key, payload, precondition)
    }

    /// ⚠️ **Delegated whole, and no merging happens.** This wrapper coalesces
    /// whole-payload writes; a streaming write is already one object being
    /// assembled, and merging two of them would interleave their parts.
    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> BoxFuture<'a, Result<Box<dyn crate::MultipartWriter<'a> + 'a>>> {
        Box::pin(async move { self.inner.open_multipart(key).await })
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>> {
        self.inner.delete(keys)
    }
}

#[cfg(test)]
mod tests {
    // The workspace denies `expect_used`; every site below is on a value
    // this module just constructed from a literal it controls.
    #![allow(clippy::expect_used)]

    use super::{MergedGroup, MergingObjectStore, plan_merge};
    use crate::{
        ByteRange, CountingObjectStore, Error, FakeObjectStore, ObjectKey, ObjectStore, Operation,
    };
    use std::future::Future;

    fn key() -> ObjectKey {
        ObjectKey::new("merge-test").expect("a non-empty key")
    }

    #[test]
    fn a_single_span_is_its_own_group() {
        assert_eq!(
            plan_merge(&[(0, 10, 20)], 0),
            vec![MergedGroup {
                start: 10,
                end: 20,
                members: vec![(0, 0, 10)],
            }]
        );
    }

    #[test]
    fn overlapping_spans_merge_regardless_of_threshold() {
        assert_eq!(
            plan_merge(&[(0, 0, 10), (1, 5, 15)], 0),
            vec![MergedGroup {
                start: 0,
                end: 15,
                members: vec![(0, 0, 10), (1, 5, 15)],
            }]
        );
    }

    #[test]
    fn a_gap_at_or_under_the_threshold_merges() {
        // [0, 10) and [15, 20): a 5-byte gap, exactly at the threshold.
        assert_eq!(
            plan_merge(&[(0, 0, 10), (1, 15, 20)], 5),
            vec![MergedGroup {
                start: 0,
                end: 20,
                members: vec![(0, 0, 10), (1, 15, 20)],
            }]
        );
    }

    #[test]
    fn a_gap_over_the_threshold_stays_separate() {
        // Same spans, one byte more gap than the threshold allows.
        assert_eq!(
            plan_merge(&[(0, 0, 10), (1, 16, 20)], 5),
            vec![
                MergedGroup {
                    start: 0,
                    end: 10,
                    members: vec![(0, 0, 10)],
                },
                MergedGroup {
                    start: 16,
                    end: 20,
                    members: vec![(1, 0, 4)],
                },
            ]
        );
    }

    #[test]
    fn a_span_nested_inside_an_earlier_one_still_merges() {
        // [0, 100) then [10, 20): starts later, entirely inside the first.
        assert_eq!(
            plan_merge(&[(0, 0, 100), (1, 10, 20)], 0),
            vec![MergedGroup {
                start: 0,
                end: 100,
                members: vec![(0, 0, 100), (1, 10, 20)],
            }]
        );
    }

    #[test]
    fn input_order_does_not_affect_the_plan() {
        let ascending = plan_merge(&[(0, 0, 10), (1, 10, 20)], 0);
        let descending = plan_merge(&[(1, 10, 20), (0, 0, 10)], 0);
        assert_eq!(ascending, descending);
    }

    /// Drives a future to completion on this thread — no async runtime,
    /// same reasoning and same shape `tests/it/store.rs`'s own `block_on`
    /// gives: every future here completes on its first poll, since nothing
    /// in this module or `FakeObjectStore` does real I/O or registers a
    /// waker. `oqueue-core` takes no async-runtime dependency, ever
    /// (ADR-0001/ADR-0002, `M0.16`) — confined to this module's own tests
    /// rather than offered as a crate-wide helper, for the same reason.
    fn block_on<F: Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut future = Box::pin(future);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    fn store_with(
        payload: Vec<u8>,
    ) -> (
        ObjectKey,
        MergingObjectStore<CountingObjectStore<FakeObjectStore>>,
    ) {
        let k = key();
        let fake = FakeObjectStore::new();
        block_on(fake.put(&k, payload, None)).expect("put succeeds");
        (
            k,
            MergingObjectStore::new(CountingObjectStore::new(fake), 0),
        )
    }

    /// ⚠️ **This is the test `backlog.md`'s `M1.19` row asks for**: a
    /// call-counting fake proves ranges within the threshold reach the
    /// backend as fewer calls than were requested.
    #[test]
    fn adjacent_ranges_within_the_threshold_cost_one_backend_call() {
        let (k, store) = store_with((0..20).collect());
        let ranges = [
            ByteRange::bounded(0, 5).expect("a valid range"),
            ByteRange::bounded(5, 5).expect("a valid range"),
        ];
        let results = block_on(store.get_many(&k, &ranges));
        assert_eq!(results[0], Ok((0..5).collect::<Vec<u8>>()));
        assert_eq!(results[1], Ok((5..10).collect::<Vec<u8>>()));
        assert_eq!(
            store.inner.counts().count(Operation::Get),
            1,
            "two adjacent ranges (zero gap) must reach the backend as one call"
        );
    }

    #[test]
    fn ranges_past_the_threshold_cost_one_backend_call_each() {
        let (k, store) = store_with((0..20).collect());
        let ranges = [
            ByteRange::bounded(0, 5).expect("a valid range"),
            ByteRange::bounded(10, 5).expect("a valid range"),
        ];
        let results = block_on(store.get_many(&k, &ranges));
        assert_eq!(results[0], Ok((0..5).collect::<Vec<u8>>()));
        assert_eq!(results[1], Ok((10..15).collect::<Vec<u8>>()));
        assert_eq!(
            store.inner.counts().count(Operation::Get),
            2,
            "a 5-byte gap against a 0 threshold must reach the backend as two calls"
        );
    }

    #[test]
    fn a_failed_group_fails_every_one_of_its_members_independently() {
        let fake = FakeObjectStore::new();
        let store = MergingObjectStore::new(CountingObjectStore::new(fake), 0);
        let k = key();
        // Nothing was ever put under `k`, so any fetch is ObjectNotFound.
        let ranges = [
            ByteRange::bounded(0, 5).expect("a valid range"),
            ByteRange::bounded(5, 5).expect("a valid range"),
        ];
        let results = block_on(store.get_many(&k, &ranges));
        assert_eq!(
            results,
            vec![
                Err(Error::ObjectNotFound { key: k.clone() }),
                Err(Error::ObjectNotFound { key: k }),
            ]
        );
    }

    #[test]
    fn a_full_range_is_never_merged_and_always_dispatched() {
        let (k, store) = store_with((0..20).collect());
        let ranges = [
            ByteRange::Full,
            ByteRange::bounded(0, 5).expect("a valid range"),
        ];
        let results = block_on(store.get_many(&k, &ranges));
        assert_eq!(results[0], Ok((0..20).collect::<Vec<u8>>()));
        assert_eq!(results[1], Ok((0..5).collect::<Vec<u8>>()));
        assert_eq!(
            store.inner.counts().count(Operation::Get),
            2,
            "Full never merges with a Bounded range, so each costs its own call"
        );
    }

    #[test]
    fn get_put_delete_forward_unchanged() {
        let fake = FakeObjectStore::new();
        let store = MergingObjectStore::new(fake, 0);
        let k = key();
        let meta = block_on(store.put(&k, vec![1, 2, 3], None)).expect("put succeeds");
        assert_eq!(meta.size, 3);
        assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(vec![1, 2, 3]));
        block_on(store.delete(std::slice::from_ref(&k))).expect("delete succeeds");
        assert_eq!(
            block_on(store.get(&k, ByteRange::Full)),
            Err(Error::ObjectNotFound { key: k })
        );
    }
}
