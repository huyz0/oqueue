//! What the two cache tests share: a future driver and a KMS call counter.

// Sites are on values these tests constructed from literals they control.
#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is the only root of this
// test binary and nothing outside it can name these. The workspace's
// `unreachable_pub = "deny"` is right about library crates and has no way to
// tell a test binary's shared module apart from one; `redundant_pub_crate`
// refuses the other spelling, so one of the two must be allowed.
#![allow(unreachable_pub)]

use oqueue_core::{BoxFuture, KeyId, KeyProvider, Redacted, Result, WrappedKey};
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

/// ⚠️ No async runtime — ADR-0002 and `M0.16`'s floor. Correct only because
/// every future reachable from here completes on its first poll:
/// [`CountingKeyProvider`] wraps [`FakeKeyProvider`](oqueue_core::FakeKeyProvider),
/// which does no I/O.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}

/// A [`KeyProvider`] that counts what it is asked to do and delegates the
/// rest.
///
/// ⚠️ **The counter is the whole assertion `NFR-33` needs.** "KMS calls are a
/// function of DEK rotation, not of produce volume" is a claim about *how many
/// times the seam was entered*, and nothing but a count at the seam can check
/// it — a test that only asserted the right plaintext came back would pass
/// against a cache that called the KMS on every region.
#[derive(Debug)]
pub struct CountingKeyProvider<P> {
    inner: P,
    wraps: AtomicUsize,
    unwraps: AtomicUsize,
}

impl<P: KeyProvider> CountingKeyProvider<P> {
    pub const fn new(inner: P) -> Self {
        Self {
            inner,
            wraps: AtomicUsize::new(0),
            unwraps: AtomicUsize::new(0),
        }
    }

    pub fn wraps(&self) -> usize {
        self.wraps.load(Ordering::SeqCst)
    }

    pub fn unwraps(&self) -> usize {
        self.unwraps.load(Ordering::SeqCst)
    }
}

impl<P: KeyProvider> KeyProvider for CountingKeyProvider<P> {
    fn wrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        plaintext: &'a Redacted<Vec<u8>>,
    ) -> BoxFuture<'a, Result<WrappedKey>> {
        // ⚠️ Counted on entry, not on success: a call that failed still cost a
        // request against the KMS's quota, which is the thing being bounded.
        self.wraps.fetch_add(1, Ordering::SeqCst);
        self.inner.wrap(key_id, plaintext)
    }

    fn unwrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        wrapped: &'a WrappedKey,
    ) -> BoxFuture<'a, Result<Redacted<Vec<u8>>>> {
        self.unwraps.fetch_add(1, Ordering::SeqCst);
        self.inner.unwrap(key_id, wrapped)
    }
}

/// The KEK every test here seals under.
pub fn key_id() -> KeyId {
    KeyId::new("arn:aws:kms:eu-west-1:123456789012:key/abcd").expect("non-empty")
}
