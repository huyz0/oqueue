//! Push for the tail: what a reader follows instead of polling.

use oqueue_core::{CommitVersion, MetadataEntry};
use tokio::sync::{broadcast, watch};

/// How many committed entries a subscription buffers before a slow subscriber
/// falls behind.
///
/// ⚠️ **A lag is designed for, not prevented.** Doc 12 §4.4's answer is push
/// for the tail and *pull* for history, and this constant is where the one
/// becomes the other: a subscriber that cannot keep up is told so
/// ([`DeltaLag::Lagged`]) and re-bootstraps from the log, which is a bounded
/// paged read rather than an unbounded buffer growing behind it. Sizing this
/// larger would trade memory for a boundary that has to exist anyway.
///
/// ⚠️ **UNDERIVED.** A placeholder with the right shape, like
/// `TAIL_WINDOW_ENTRIES` beside the index; `M14` measures what it should be.
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
///
/// ⚠️ **It must stay a power of two, and the reason is not this crate's.**
/// `tokio::sync::broadcast` rounds a channel's capacity **up** to the next
/// power of two, so a value of, say, 1,500 would buffer 2,048 and this
/// constant would stop describing what it names — a per-shard memory figure
/// derived from it under NFR-11 would be out by up to 2×, and the lag test
/// below would stop overrunning the buffer it exists to overrun. Whoever
/// replaces the placeholder inherits that constraint.
pub const DELTA_BUFFER_ENTRIES: usize = 1_024;

/// Why a [`DeltaStream`] stopped delivering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeltaLag {
    /// The subscriber fell behind and entries it never saw were dropped.
    ///
    /// ⚠️ **Not an error to retry — an instruction to re-bootstrap.** The
    /// stream is still live and the *next* `recv` resumes from the oldest
    /// entry still buffered, but the gap in between is real, and folding what
    /// comes next onto an index missing the middle is the double-counting
    /// [`Error::NonMonotonicCommitVersion`](oqueue_core::Error::NonMonotonicCommitVersion)
    /// exists to refuse. Pull the delta from the log first — doc 12 §4.4's
    /// history half — and then follow again.
    #[error("the subscription fell behind by {missed} entries; re-bootstrap from the log")]
    Lagged {
        /// How many entries were dropped.
        missed: u64,
    },

    /// Every publisher is gone. Nothing further will arrive.
    ///
    /// ⚠️ **It fires whether or not a handle is still held**, and that took a
    /// deliberate choice: `broadcast` reports this only once every *sender* is
    /// gone, so a [`Coordinator`](crate::Coordinator) that kept one would park
    /// a follower forever against a loop that had already stopped. The loop
    /// owns the only sender — exactly as it owns the only
    /// [`IndexWatch`] sender — so a stopped loop is reported the same way by
    /// both, and a handle in some connection task cannot mask it.
    #[error("the coordinator is no longer publishing")]
    Closed,
}

/// A subscription to the tail of one metadata shard's log.
///
/// ⚠️ **Deltas, not state.** What arrives is the same event-shaped
/// [`MetadataEntry`] the log holds, so a follower's index is the identical
/// fold whether it was pushed or pulled — which is what makes a re-bootstrap
/// after [`DeltaLag::Lagged`] converge rather than merely resynchronize.
#[derive(Debug)]
pub struct DeltaStream {
    deltas: broadcast::Receiver<MetadataEntry>,
}

impl DeltaStream {
    pub(crate) const fn new(deltas: broadcast::Receiver<MetadataEntry>) -> Self {
        Self { deltas }
    }

    /// The next committed entry.
    ///
    /// ⚠️ **Cancellation-safe**, unlike
    /// [`Coordinator::commit`](crate::Coordinator::commit): dropping this
    /// future loses nothing, because the entry stays in the buffer until this
    /// receiver takes it. That is what lets a fetch `select!` it against a
    /// deadline, which is the whole of `M3.md` task 17.
    ///
    /// # Errors
    ///
    /// [`DeltaLag::Lagged`] if entries were dropped — re-bootstrap.
    /// [`DeltaLag::Closed`] if the coordinator has stopped.
    pub async fn recv(&mut self) -> Result<MetadataEntry, DeltaLag> {
        match self.deltas.recv().await {
            Ok(entry) => Ok(entry),
            Err(broadcast::error::RecvError::Lagged(missed)) => Err(DeltaLag::Lagged { missed }),
            Err(broadcast::error::RecvError::Closed) => Err(DeltaLag::Closed),
        }
    }
}

/// How far the coordinator's own index has been folded.
///
/// ⚠️ **The index's version, not the log's**, and the difference is the whole
/// point. A reader parked for its own write needs the position to be
/// *queryable*, not merely durable; waking it when the log had the entry and
/// the index did not would answer a fetch from an index that has not folded
/// the record the fetch is waiting for — hazard H2 restored with an extra step.
#[derive(Debug, Clone)]
pub struct IndexWatch {
    applied: watch::Receiver<Option<CommitVersion>>,
}

impl IndexWatch {
    pub(crate) const fn new(applied: watch::Receiver<Option<CommitVersion>>) -> Self {
        Self { applied }
    }

    /// The highest version the coordinator's index holds, or `None`.
    #[must_use]
    pub fn applied(&self) -> Option<CommitVersion> {
        *self.applied.borrow()
    }

    /// Waits until the index holds `version`, and says whether it does.
    ///
    /// Returns `true` immediately if it already does. `M3.md` task 17: a fetch
    /// parks on this and on its own `fetch.max.wait.ms` deadline, so freshness
    /// costs a wakeup rather than a poll interval, and **no new semantics** —
    /// the deadline is the caller's, because the timer is the caller's.
    ///
    /// ⚠️ **Cancellation-safe.** Dropping it changes nothing; the watch is
    /// level-triggered, so a later wait sees the same state rather than
    /// missing an edge.
    ///
    /// ⚠️ **`false` means the coordinator stopped**, not that the version will
    /// never arrive by some other route. It returns rather than parking on so
    /// that a fetch answers from what the index already has instead of waiting
    /// out a deadline nothing can now satisfy — and it is a `bool` rather than
    /// a bare resolve so that "I waited" and "it arrived" cannot be confused,
    /// which for an `AtLeast(v)` read is the difference between an answer and
    /// hazard H2.
    pub async fn wait_for(&mut self, version: CommitVersion) -> bool {
        // ⚠️ `wait_for` on the receiver, not a `changed()` loop: it evaluates
        // the predicate against the *current* value first, so a version
        // already applied returns without waiting for a further change that
        // may never come. The borrow it hands back is dropped at the end of
        // this statement — holding it across an await would make this future
        // `!Send`, and every caller is a spawned fetch.
        self.applied
            .wait_for(|applied| applied.is_some_and(|applied| applied >= version))
            .await
            .is_ok()
    }

    /// Waits until the index has folded anything at all past `applied`.
    ///
    /// ⚠️ **What a long-poll fetch waits on, and it is not
    /// [`wait_for`](Self::wait_for).** A fetch at the high watermark is not
    /// waiting for a version it can name — it is waiting for the *next* one,
    /// whatever that turns out to be, and a client's `fetch.max.wait.ms` is
    /// the only bound on how long. `M3.md` task 17: the wakeup costs about a
    /// millisecond and introduces no new semantics, because the deadline stays
    /// the caller's.
    ///
    /// ⚠️ **It may wake for a partition the caller does not care about.** One
    /// index serves every partition on a shard, so a delta for any of them
    /// resolves this; the caller re-reads, finds nothing, and parks again
    /// against its own remaining deadline. That is a wasted wakeup, never a
    /// wrong answer — and it is what a per-partition condition would trade for
    /// a watch per partition.
    ///
    /// ⚠️ **Cancellation-safe**, for the same reason
    /// [`wait_for`](Self::wait_for) is: the watch is level-triggered, so a
    /// dropped wait loses no edge.
    ///
    /// Returns `false` if the coordinator stopped, so a parked fetch answers
    /// from what the index already holds rather than waiting out a deadline
    /// nothing can satisfy.
    pub async fn wait_past(&mut self, applied: Option<CommitVersion>) -> bool {
        self.applied
            .wait_for(|current| match (current, applied) {
                // Anything at all is past "nothing folded yet".
                (Some(_), None) => true,
                (Some(current), Some(applied)) => *current > applied,
                (None, _) => false,
            })
            .await
            .is_ok()
    }
}
