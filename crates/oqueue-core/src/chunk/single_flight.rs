//! The single-flight machinery behind [`super::ChunkedObjectStore`]: what
//! identifies a chunk, where one fetch's outcome is shared, and how a
//! follower waits for a leader.
//!
//! ⚠️ **Separated from `chunk.rs` by concept, not by line count.** The parent
//! module is about addressing reads in aligned chunks; this one is about one
//! caller doing work several are waiting on. They grew together and crossed
//! `code-structure.md`'s 500-line limit together, which rule 18 calls a design
//! signal rather than a formatting problem — it was right, and this is the
//! seam it was pointing at.

use crate::{Error, ObjectKey, Result};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

/// Identifies one chunk-aligned read: which object, which chunk index, and
/// how long the chunk actually is — usually `chunk_size`, but shorter for
/// the last, partial chunk of an object, which is why length is part of the
/// key rather than assumed: two callers asking for the same `chunk_index`
/// with different lengths are not asking for the same bytes and must not
/// share an answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ChunkKey {
    pub(super) key: ObjectKey,
    pub(super) chunk_index: u64,
    pub(super) length: u64,
}

/// The shared outcome of one in-flight (or just-finished) chunk fetch.
/// `result` is `None` for as long as the leader's fetch is still running;
/// every follower registers its [`Waker`] here and is woken once it is set.
pub(super) struct Shared {
    pub(super) result: Option<Result<Vec<u8>>>,
    pub(super) wakers: Vec<Waker>,
}

/// ⚠️ Prints whether the fetch has resolved, never the bytes it resolved to —
/// and `ChunkedObjectStore`'s own derived `Debug` reaches this through the
/// registry, so a derive here would put object payloads one `{store:?}` away
/// from a log. Exactly the hole [`crate::FakeObjectStore`]'s hand-written
/// `Debug` in `store.rs` exists to close, closed the same way.
impl core::fmt::Debug for Shared {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Shared")
            .field("resolved", &self.result.is_some())
            .field("wakers", &self.wakers.len())
            .finish()
    }
}

/// A follower's future: ready the moment [`Shared::result`] is set, whoever
/// sets it — the leader's own success path, or [`LeaderGuard`]'s `Drop` if
/// the leader is cancelled first.
pub(super) struct Join {
    pub(super) shared: Arc<Mutex<Shared>>,
}

impl Future for Join {
    type Output = Result<Vec<u8>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(result) = &shared.result {
            return Poll::Ready(result.clone());
        }
        // ⚠️ Only once per distinct waker. An executor may poll a pending
        // future any number of times before it completes (a spurious wake is
        // legal), and pushing a clone on each poll would grow this vector
        // without bound for as long as the leader's fetch runs.
        if !shared.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            shared.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}

/// Owned by the one caller driving a chunk's real fetch. Publishing the
/// result — to the registry's next caller and to every current follower —
/// happens exactly once, whether the leader finishes normally
/// ([`LeaderGuard::finish`]) or is dropped before finishing (`Drop`, e.g. a
/// caller's own `select!`/timeout cancelling this future mid-fetch): a
/// cancelled leader must not strand every follower `Pending` forever.
pub(super) struct LeaderGuard<'a> {
    pub(super) registry: &'a Mutex<HashMap<ChunkKey, Arc<Mutex<Shared>>>>,
    pub(super) key: ChunkKey,
    pub(super) shared: Arc<Mutex<Shared>>,
    pub(super) done: bool,
}

impl LeaderGuard<'_> {
    /// Publishes `result` as this chunk's answer and consumes the guard —
    /// its `Drop` sees `done` and does not publish a second time.
    pub(super) fn finish(mut self, result: Result<Vec<u8>>) -> Result<Vec<u8>> {
        self.publish(result.clone());
        self.done = true;
        result
    }

    fn publish(&self, result: Result<Vec<u8>>) {
        // Removed from the registry first: a call arriving after this point
        // starts its own fresh fetch rather than joining one that has
        // already finished (or been cancelled).
        self.registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
        let wakers = {
            let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
            shared.result = Some(result);
            std::mem::take(&mut shared.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }
}

impl Drop for LeaderGuard<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.publish(Err(Error::Transient));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::Shared;

    /// ⚠️ Pins the hand-written `Debug` the way `tests/it/store.rs` pins
    /// [`crate::FakeObjectStore`]'s, and for the same reason: a payload is
    /// customer data, `ChunkedObjectStore`'s derived `Debug` reaches
    /// [`Shared`] through the registry, and a derive here would put those
    /// bytes one `{store:?}` away from a log.
    #[test]
    fn shared_debug_reports_state_and_never_payload_bytes() {
        let shared = Shared {
            result: Some(Ok(vec![0xAB; 4])),
            wakers: Vec::new(),
        };
        let rendered = format!("{shared:?}");
        assert!(rendered.contains("resolved: true"), "{rendered}");
        assert!(rendered.contains("wakers: 0"), "{rendered}");
        // `Vec<u8>`'s own `Debug` renders 0xAB as decimal 171.
        assert!(
            !rendered.contains("171"),
            "payload bytes reached Debug: {rendered}"
        );
    }
}
