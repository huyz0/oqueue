//! The one place in this repository that reads the real clock.
//!
//! ⚠️ **Here because `AGENTS.md` non-negotiable 5 puts it here.** A library
//! crate may not read the real clock — `check-sans-io.sh` is the gate — and
//! [`Clock`] is the seam that exists so it does not have to. Everything
//! upstream takes an `Arc<dyn Clock>`; this is the process that supplies a
//! real one, exactly as it is the process that supplies a real object store.
//!
//! ⚠️ **`M5.86` is why one was needed at all.** Until a commit carried the
//! moment the log took it, nothing in the write path asked what time it was,
//! and `FakeClock` was the only implementation in the tree.

// `pub(crate)` is the visibility that is true — nothing outside this binary
// may name it — so the lint that prefers `pub` is the one allowed, the call
// `security.rs` makes for the same reason.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{Clock, Timestamp};
use std::time::{SystemTime, UNIX_EPOCH};

/// A [`Clock`] reading the host's wall clock.
#[derive(Debug, Default)]
pub(crate) struct WallClock;

impl Clock for WallClock {
    /// ⚠️ **A clock before the epoch reads as the epoch, and does not panic.**
    /// `Timestamp` refuses a negative millisecond (`Error::NegativeTimestamp`)
    /// and this returns one infallibly, so the two have to be reconciled
    /// somewhere. A host whose clock is set before 1970 is misconfigured, and
    /// the choice is between refusing every commit and stamping them at the
    /// floor — the floor, because retention reading "very old" for data that
    /// is in fact new is a bounded wrong answer that an operator can see,
    /// while a broker that will not accept a produce is an outage.
    ///
    /// ⚠️ **It is not monotone and nothing here pretends it is.** A wall clock
    /// steps backwards over NTP corrections, so two commits can carry
    /// timestamps out of order with their commit versions. Retention reads a
    /// per-partition extent and is insensitive to that; anything needing
    /// order uses [`CommitVersion`](oqueue_core::CommitVersion), which is what
    /// `ADR-0020` makes every staleness comparison rest on.
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|since| i64::try_from(since.as_millis()).ok())
            .unwrap_or(0);
        Timestamp::from_millis(millis).unwrap_or(Timestamp::EPOCH)
    }
}
