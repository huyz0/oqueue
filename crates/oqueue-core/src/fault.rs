//! Fault injection for [`crate::FakeObjectStore`].
//!
//! ⚠️ **Every fault defaults to off.** [`FaultConfig::default`] reproduces
//! `M1.6`'s behaviour exactly — a test that never asks for a fault sees none,
//! which is what lets every test written before this module keep passing
//! unchanged.

use crate::Error;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Which class of transient failure a fault-injection storm produces.
///
/// ⚠️ **Not [`crate::Error::Permanent`] or [`crate::Error::PreconditionFailed`].**
/// Neither fits "storm" — a permanent failure does not resolve by retrying,
/// and a precondition failure is a property of the call, not of the backend's
/// mood. The fake is the first thing in this workspace to *produce* these
/// three variants; the fourth (`Permanent`) waits for a real backend, since
/// nothing about a fault-injection storm resembles the shape of an
/// unretryable client error (`error-handling.md` rule 6 — a variant nothing
/// can produce is not added for completeness).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StormKind {
    /// The backend is asking the caller to slow down.
    SlowDown,
    /// The backend rejected the request as over its rate limit.
    Throttled,
    /// A generic transient backend failure.
    Transient,
}

impl StormKind {
    pub(crate) const fn into_error(self) -> Error {
        match self {
            Self::SlowDown => Error::SlowDown,
            Self::Throttled => Error::Throttled,
            Self::Transient => Error::Transient,
        }
    }
}

/// Fault-injection configuration for [`crate::FakeObjectStore`].
///
/// Every field is a knob, and every knob starts off. Build one with
/// [`FaultConfig::default`] and set only the fields a test needs — an
/// explicit opt-in, never an implicit one.
#[derive(Debug, Clone, Copy, Default)]
pub struct FaultConfig {
    /// Every `get`/`put`/`delete` call returns [`Poll::Pending`] this many
    /// times before resolving — a deterministic stand-in for latency.
    /// ⚠️ **Not real time.** Nothing in this crate compiles a clock or a
    /// timer (`NFR-56`'s floor), and `tdd.md`'s no-flake rule bans `sleep`
    /// outright — a fixed poll count is controllable and adds no wall-clock
    /// cost to a test that sets it.
    pub latency_polls: u32,
    /// If `Some((kind, n))`, the next `n` calls (across every key, any of
    /// `get`/`put`/`delete`) fail with `kind` before the fake goes back to
    /// succeeding — a "503 storm": a burst of backend unavailability, not a
    /// property of any one key.
    pub storm: Option<(StormKind, u32)>,
    /// The next `n` successful `put`s durably write the object but still
    /// resolve `Err(Error::Transient)` — the "unknown state" ADR-0005
    /// documents: a caller cannot treat a failed `put` as proof of absence,
    /// because the object may have landed anyway.
    pub crash_after_put_before_ack: u32,
}

/// Resolves by evaluating `f`, after returning [`Poll::Pending`] `remaining`
/// times — waking itself each time, so a real executor does not hang waiting
/// for a wakeup nothing will ever send.
///
/// ⚠️ **This is the only mechanism this crate has for simulated latency**,
/// and deliberately so: a poll count is deterministic and instant to run,
/// where a real delay would be neither.
///
/// ⚠️ **`pub(crate)`, with an explicit allow.** `clippy::redundant_pub_crate`
/// (nursery) wants `pub` since this module is private and `pub(crate)` reads
/// as redundant from inside it; `unreachable_pub` (workspace-wide, denied)
/// wants `pub(crate)` since a bare `pub` here is not actually reachable from
/// outside the crate. The two lints want opposite things for exactly this
/// shape — a crate-internal type in a private module — so one is allowed
/// with the reason recorded, per `AGENTS.md`'s rule that a silent `#[allow]`
/// is indistinguishable from an oversight.
#[allow(clippy::redundant_pub_crate)]
pub(crate) struct DelayedThen<T, F: FnOnce() -> T> {
    remaining: u32,
    f: Option<F>,
}

impl<T, F: FnOnce() -> T> DelayedThen<T, F> {
    pub(crate) const fn new(remaining: u32, f: F) -> Self {
        Self {
            remaining,
            f: Some(f),
        }
    }
}

impl<T, F: FnOnce() -> T + Unpin> Future for DelayedThen<T, F> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if self.remaining > 0 {
            self.remaining -= 1;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        // ⚠️ `unreachable!`, not `.expect()` — the latter is denied outside
        // tests (`rust-style.md` rule 6). `self.f` is `None` only once `f()`
        // has already run and produced `Ready` — the one `take()` in this
        // function — so reaching `None` here means this future was polled
        // again after resolving, which `Future`'s own contract says a caller
        // must not do.
        let Some(f) = self.f.take() else {
            unreachable!("DelayedThen polled again after resolving");
        };
        Poll::Ready(f())
    }
}
