//! Reads addressed in fixed-size, aligned chunks, with concurrent readers of
//! the *same* chunk sharing one in-flight backend fetch rather than issuing
//! their own.
//!
//! ⚠️ **Genuinely concurrent, unlike [`crate::MergingObjectStore`].**
//! `M1.19`'s merging is a caller-known batch, resolved synchronously before
//! any request is sent — it needed no real cross-task coordination. This
//! does: many callers, arriving independently and asynchronously, must be
//! able to join a fetch already under way. That is possible without a clock
//! or a runtime dependency because the join target is always an **exact**
//! match — the same `(key, chunk_index, length)` — never a fuzzy "nearby"
//! one, so there is no debounce window to build and no union to widen after
//! a fetch has already started; a hand-written [`Future`] with a shared,
//! mutex-guarded result cell is enough, the same "no async runtime" posture
//! [`crate::fault::DelayedThen`] and [`crate::BoxFuture`] already hold to.

use crate::{
    BoxFuture, ByteRange, Error, ObjectKey, ObjectMeta, ObjectStore, Precondition, Result,
};
use single_flight::{ChunkKey, Join, LeaderGuard, Shared};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, PoisonError};

mod single_flight;

/// Wraps any [`ObjectStore`], adding [`ChunkedObjectStore::get_chunk`].
///
/// Reads are addressed by `(key, chunk_index)` rather than an arbitrary byte
/// range, with concurrent callers of the same chunk sharing one backend
/// fetch. Implements [`ObjectStore`] itself, forwarding `get`/`put`/`delete`
/// unchanged, so it composes with [`crate::CountingObjectStore`] and
/// [`crate::MergingObjectStore`] (in any order) exactly like any other
/// decorator over the seam.
#[derive(Debug)]
pub struct ChunkedObjectStore<S> {
    inner: S,
    chunk_size: NonZeroU64,
    in_flight: Mutex<HashMap<ChunkKey, Arc<Mutex<Shared>>>>,
}

impl<S> ChunkedObjectStore<S> {
    /// Wraps `inner`, addressing reads in `chunk_size`-aligned chunks.
    #[must_use]
    pub fn new(inner: S, chunk_size: NonZeroU64) -> Self {
        Self {
            inner,
            chunk_size,
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Joins the fetch already in flight for `chunk_key`, or registers this
    /// caller as the one that will run it. Returns the shared result cell and
    /// whether this caller is the leader.
    ///
    /// ⚠️ **One lock acquisition covers both the lookup and the insert**,
    /// which is what makes "exactly one leader per chunk" true rather than
    /// likely: splitting them would leave a window in which two callers each
    /// find nothing and each insert.
    // ⚠️ `option_if_let_else` wants `registry.get(...).map_or_else(...)`, but
    // the "not found" arm needs to *insert* into `registry` while the "found"
    // arm still holds the borrow `.get()` returned — not a shape `map_or_else`
    // can express without fighting NLL harder than an `if let` does.
    #[allow(clippy::option_if_let_else)]
    fn join_or_lead(&self, chunk_key: &ChunkKey) -> (Arc<Mutex<Shared>>, bool) {
        let mut registry = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let outcome = if let Some(existing) = registry.get(chunk_key) {
            (Arc::clone(existing), false)
        } else {
            let shared = Arc::new(Mutex::new(Shared {
                result: None,
                wakers: Vec::new(),
            }));
            registry.insert(chunk_key.clone(), Arc::clone(&shared));
            (shared, true)
        };
        // Explicitly, not by falling out of scope: the lock is released before
        // the caller does anything with what it got back.
        drop(registry);
        outcome
    }
}

impl<S: ObjectStore> ChunkedObjectStore<S> {
    /// Fetches chunk `chunk_index` of `key`, `length` bytes long — normally
    /// this store's own `chunk_size`, but shorter for an object's last,
    /// partial chunk. The caller is expected to know the object's size
    /// already, from whatever index tracks it: this method could only learn
    /// it by issuing a `HEAD` of its own, and a decorator that exists to turn
    /// N reads into one must not spend an extra request per read to do it.
    ///
    /// A call already in flight for the same `(key, chunk_index, length)`
    /// is joined rather than duplicated: only the first caller actually
    /// invokes the wrapped store; every concurrent caller after it shares
    /// that one answer.
    ///
    /// # Errors
    ///
    /// [`Error::ChunkLengthTooLarge`] if `length` exceeds this store's chunk
    /// size, and [`Error::EmptyByteRange`] if it is zero — neither is a chunk
    /// this store addresses, and ⚠️ **both are rejected before the join key is
    /// built, not merely before the request is sent**: `length` is part of
    /// that key, so an unchecked one does not just issue a wrong-sized `GET`,
    /// it silently partitions callers of the same chunk into groups that
    /// never share a fetch — single-flight degrading to nothing, with every
    /// test still green.
    ///
    /// ⚠️ **A door check, and weaker than what the rest of this crate gets.**
    /// `lib.rs` describes every identifier here as *unconstructible-around*,
    /// which is explicitly "not merely checked at the door" — a private field
    /// and a validating constructor. `length` is a plain `u64` parameter, so
    /// it cannot have that property; the check is the door, and it is only
    /// load-bearing because nothing reads `length` before it. Two callers
    /// naming the same chunk with *different but individually legal* lengths
    /// still do not share a fetch, and that is not closable here: only the
    /// object's size says which length is the right one, and this type does
    /// not know it.
    ///
    /// [`Error::ByteRangeOutOfBounds`] if `chunk_index * chunk_size`
    /// overflows `u64` — an address no real object could ever have.
    /// Otherwise, whatever the wrapped store's `get` returns.
    pub async fn get_chunk(
        &self,
        key: &ObjectKey,
        chunk_index: u64,
        length: u64,
    ) -> Result<Vec<u8>> {
        if length > self.chunk_size.get() {
            return Err(Error::ChunkLengthTooLarge {
                length,
                chunk_size: self.chunk_size.get(),
            });
        }
        let Some(start) = chunk_index.checked_mul(self.chunk_size.get()) else {
            return Err(Error::ByteRangeOutOfBounds {
                key: key.clone(),
                offset: u64::MAX,
                length,
                object_size: u64::MAX,
            });
        };
        let range = ByteRange::bounded(start, length)?;

        let chunk_key = ChunkKey {
            key: key.clone(),
            chunk_index,
            length,
        };

        let (shared, is_leader) = self.join_or_lead(&chunk_key);

        if !is_leader {
            return Join { shared }.await;
        }

        let guard = LeaderGuard {
            registry: &self.in_flight,
            key: chunk_key,
            shared,
            done: false,
        };
        let result = self.inner.get(key, range).await;
        guard.finish(result)
    }
}

impl<S: ObjectStore> ObjectStore for ChunkedObjectStore<S> {
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

    /// ⚠️ **Delegated whole, and chunking does not apply.** This wrapper
    /// exists to hold a `put` to the store's chunk size; a streaming writer's
    /// caller decides what a part is, and imposing a second chunking on top
    /// would split parts the caller sized against
    /// [`MultipartLimits`](crate::MultipartLimits) into ones it did not.
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

    use super::ChunkedObjectStore;
    use crate::test_executor::{block_on, run_all};
    use crate::{
        ByteRange, CountingObjectStore, Error, FakeObjectStore, ObjectKey, ObjectStore, Operation,
    };
    use std::num::NonZeroU64;
    use std::task::{Context, Poll, Waker};

    fn key() -> ObjectKey {
        ObjectKey::new("chunk-test").expect("a non-empty key")
    }

    const CHUNK_SIZE: NonZeroU64 = NonZeroU64::new(4).expect("non-zero");

    fn store_with(
        payload: Vec<u8>,
    ) -> (
        ObjectKey,
        ChunkedObjectStore<CountingObjectStore<FakeObjectStore>>,
    ) {
        let k = key();
        let fake = FakeObjectStore::new();
        block_on(fake.put(&k, payload, None)).expect("put succeeds");
        (
            k,
            ChunkedObjectStore::new(CountingObjectStore::new(fake), CHUNK_SIZE),
        )
    }

    #[test]
    fn get_chunk_reads_the_chunk_aligned_range() {
        let (k, store) = store_with(vec![0, 1, 2, 3, 4, 5, 6, 7]);
        // chunk_size 4: chunk 1 is bytes [4, 8).
        assert_eq!(block_on(store.get_chunk(&k, 1, 4)), Ok(vec![4, 5, 6, 7]));
    }

    #[test]
    fn get_chunk_supports_a_shorter_final_chunk() {
        let (k, store) = store_with(vec![0, 1, 2, 3, 4, 5]);
        // chunk_size 4: chunk 1 is only 2 bytes long here, [4, 6).
        assert_eq!(block_on(store.get_chunk(&k, 1, 2)), Ok(vec![4, 5]));
    }

    #[test]
    fn chunk_index_overflow_is_out_of_bounds_not_a_panic() {
        let (k, store) = store_with(vec![0; 4]);
        assert_eq!(
            block_on(store.get_chunk(&k, u64::MAX, 4)),
            Err(Error::ByteRangeOutOfBounds {
                key: k,
                offset: u64::MAX,
                length: 4,
                object_size: u64::MAX,
            })
        );
    }

    #[test]
    fn distinct_chunks_each_cost_their_own_backend_call() {
        let (k, store) = store_with(vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(block_on(store.get_chunk(&k, 0, 4)), Ok(vec![0, 1, 2, 3]));
        assert_eq!(block_on(store.get_chunk(&k, 1, 4)), Ok(vec![4, 5, 6, 7]));
        assert_eq!(store.inner.counts().count(Operation::Get), 2);
    }

    /// ⚠️ **This is the test `backlog.md`'s `M1.20` row asks for**: 500
    /// concurrent readers of the exact same chunk, proven — via a
    /// call-counting fake, not inferred from timing — to cost far fewer than
    /// 500 backend fetches. It proves **exactly one**, which is the real
    /// property; the row's "far fewer" was written before there was an
    /// implementation to be precise about.
    ///
    /// ⚠️ **Deterministic, and deliberately not 500 OS threads.** The first
    /// version of this test spawned real threads, and was flaky: with no
    /// latency configured `FakeObjectStore::get` resolves on its first poll,
    /// so a leader could finish and deregister before the second thread was
    /// even scheduled — 500/500 backend calls, no dedup at all, intermittently.
    /// A `Barrier` plus injected latency made it pass 20 runs in a row, but
    /// that only made the race *likely* to go the right way, never certain,
    /// and it left the test unable to fail fast: under mutation the followers
    /// spun forever rather than failing (see [`run_all`]).
    ///
    /// Polling 500 futures on one thread is what makes the interleaving a
    /// fact rather than a hope. `latency_polls: 1` is exactly enough: the
    /// leader is still in flight when the other 499 are first polled, because
    /// [`run_all`] polls the whole batch before returning to it. That is the
    /// same concurrency the seam actually sees in production — many tasks on
    /// an executor — and `tdd.md`'s no-flake rule prefers it to real threads
    /// wherever the property does not require them.
    #[test]
    fn five_hundred_concurrent_readers_of_the_same_chunk_share_one_backend_call() {
        use crate::FaultConfig;

        let k = key();
        let fake = FakeObjectStore::with_faults(FaultConfig {
            latency_polls: 1,
            ..FaultConfig::default()
        });
        block_on(fake.put(&k, vec![0xAB; 4], None)).expect("put succeeds");
        let store = ChunkedObjectStore::new(CountingObjectStore::new(fake), CHUNK_SIZE);

        let readers: Vec<_> = (0..500).map(|_| store.get_chunk(&k, 0, 4)).collect();
        for result in run_all(readers) {
            assert_eq!(result, Ok(vec![0xAB; 4]));
        }

        assert_eq!(
            store.inner.counts().count(Operation::Get),
            1,
            "500 concurrent readers of the same chunk must share one backend fetch"
        );
    }

    /// Exercises [`LeaderGuard`]'s `Drop` directly: a leader cancelled before
    /// it finishes (dropped mid-fetch, e.g. by a caller's own timeout) must
    /// still wake every follower already registered — with an error, not by
    /// silently leaving them `Pending` forever. Needs manual polling rather
    /// than `block_on`, since the point is to stop polling the leader
    /// partway through and drop it while a follower is still waiting.
    #[test]
    fn dropping_the_leader_before_it_finishes_wakes_followers_with_an_error() {
        use crate::FaultConfig;

        let k = key();
        let fake = FakeObjectStore::with_faults(FaultConfig {
            latency_polls: 10,
            ..FaultConfig::default()
        });
        block_on(fake.put(&k, vec![0xAB; 4], None)).expect("put succeeds");
        let store = ChunkedObjectStore::new(fake, CHUNK_SIZE);

        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);

        // Leader: one poll is enough to register it and start (and, with
        // latency configured, stay pending on) the real fetch.
        let mut leader = Box::pin(store.get_chunk(&k, 0, 4));
        assert_eq!(leader.as_mut().poll(&mut cx), Poll::Pending);

        // Follower: registers itself against the same in-flight chunk.
        let mut follower = Box::pin(store.get_chunk(&k, 0, 4));
        assert_eq!(follower.as_mut().poll(&mut cx), Poll::Pending);

        // Cancel the leader before it ever finishes.
        drop(leader);

        // The follower is woken with an error rather than stranded.
        assert_eq!(
            follower.as_mut().poll(&mut cx),
            Poll::Ready(Err(Error::Transient))
        );
    }

    #[test]
    fn a_length_over_the_chunk_size_is_rejected_at_the_door() {
        let (k, store) = store_with(vec![0; 8]);
        assert_eq!(
            block_on(store.get_chunk(&k, 0, 5)),
            Err(Error::ChunkLengthTooLarge {
                length: 5,
                chunk_size: 4,
            })
        );
        // ⚠️ Rejected *before* the backend, not after: an over-long length
        // must not cost a request either.
        assert_eq!(store.inner.counts().count(Operation::Get), 0);
    }

    #[test]
    fn a_zero_length_is_rejected_at_the_door() {
        let (k, store) = store_with(vec![0; 8]);
        assert_eq!(
            block_on(store.get_chunk(&k, 0, 0)),
            Err(Error::EmptyByteRange)
        );
        assert_eq!(store.inner.counts().count(Operation::Get), 0);
    }

    /// ⚠️ `get_chunk` is a `pub async fn`, so its future's `Send`-ness is
    /// inferred rather than declared — unlike [`crate::BoxFuture`], which
    /// `store.rs` pins as `+ Send`. A caller spawning it as a task needs
    /// `Send`, and nothing else in the tree would notice if a future field
    /// quietly took it away, because these tests all poll on one thread.
    #[test]
    fn the_returned_future_is_send() {
        const fn assert_send<T: Send>(_: &T) {}

        let (k, store) = store_with(vec![0, 1, 2, 3]);
        let reader = store.get_chunk(&k, 0, 4);
        assert_send(&reader);
        assert_eq!(block_on(reader), Ok(vec![0, 1, 2, 3]));
    }

    #[test]
    fn get_put_delete_forward_unchanged() {
        let fake = FakeObjectStore::new();
        let store = ChunkedObjectStore::new(fake, CHUNK_SIZE);
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
