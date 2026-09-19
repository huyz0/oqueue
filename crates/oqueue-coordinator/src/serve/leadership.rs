//! Whether the loop may write, and from what view of the log: the lease it is
//! fenced by, the deferred replay, and the replay under each new term.
//!
//! ⚠️ **Split from `serve.rs` at the 500-line limit, along the concept**
//! (`M6.17`): that file serves requests; this one decides whether a request
//! may be served at all, and makes the allocator current before it is.

// ⚠️ `pub(super)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use std::sync::Arc;

use oqueue_core::{MetadataEntry, ObjectStoreLease};

use super::{CoordinatorLoop, Reopen, Reopener};
use crate::error::CoordinatorError;

impl CoordinatorLoop {
    /// Fences every write on `lease`: a commit or trim arriving while it is
    /// not held is refused with [`CoordinatorError::Fenced`] (`M6.7`).
    #[must_use]
    pub fn fenced_by(mut self, lease: Arc<ObjectStoreLease>) -> Self {
        self.lease = Some(lease);
        self
    }

    /// Replays from a freshly opened view of the log whenever the loop is
    /// about to write under a lease term it did not replay under (`M6.17`).
    ///
    /// ⚠️ **What this closes is a data-loss path**, found by M6's closing
    /// review. A node replays its log once, at boot; if it took the lease only
    /// later, its allocator and its log's next segment were the boot-time
    /// ones. Had the previous leader checkpointed meanwhile, that segment key
    /// was deleted — absent again — so the stale leader's create-only write
    /// landed behind the new base, at offsets already acknowledged, where no
    /// later open reads. Replaying under each new term, from a view opened
    /// after the lease was taken, makes the allocator and the log both
    /// current before the first write.
    #[must_use]
    pub fn reopening_with(mut self, reopen: Reopen) -> Self {
        self.reopen = Some(Reopener(reopen));
        self
    }

    /// Replays afresh if this loop now leads under a term it has not
    /// replayed under; whether it is current.
    pub(super) async fn current_for_term(&mut self) -> bool {
        let (Some(lease), Some(reopen)) = (&self.lease, &self.reopen) else {
            return true;
        };
        let term = lease.term();
        if term.is_some() && term == self.replayed_term {
            return true;
        }
        let Ok(log) = (reopen.0)().await else {
            return false;
        };
        let Ok((allocator, last)) = self.refold(&*log).await else {
            return false;
        };
        self.log = log;
        self.allocator = allocator;
        self.last_committed = last;
        self.replayed_term = term;
        self.publish_applied();
        true
    }

    /// A fresh allocator folded from the whole of `log`, and the index
    /// advanced by only what it lacks.
    ///
    /// ⚠️ **The index is never cleared here** (`M6.17`'s review): readers are
    /// serving from it, and it is already a prefix of this same log, so what a
    /// new term needs is its missing suffix. A failure partway leaves it a
    /// longer prefix, never an emptier one; the allocator, which no reader
    /// sees, is rebuilt in private and taken only if the whole fold lands.
    async fn refold(
        &self,
        log: &dyn oqueue_core::MetadataLog,
    ) -> Result<
        (
            crate::allocator::Allocator,
            Option<oqueue_core::CommitVersion>,
        ),
        (),
    > {
        let mut allocator = crate::allocator::Allocator::new();
        let mut last = None;
        let mut from = oqueue_core::CommitVersion::ZERO;
        loop {
            let page = log
                .read_from(from, super::REBUILD_PAGE_ENTRIES)
                .await
                .map_err(|_| ())?;
            let Some(tail) = page.last() else {
                return Ok((allocator, last));
            };
            for entry in &page {
                allocator.replay(entry).map_err(|_| ())?;
            }
            let held = self.index.applied_upto();
            let missing: Vec<MetadataEntry> = page
                .iter()
                .filter(|entry| held.is_none_or(|held| entry.version() > held))
                .cloned()
                .collect();
            // An empty apply is a no-op (`MaterializedIndex` guarantee).
            self.index.apply(&missing).map_err(|_| ())?;
            last = Some(tail.version());
            from = tail.version().advance(1).map_err(|_| ())?;
        }
    }

    /// Appends one entry. ⚠️ **A write the log's own fence refused means
    /// another writer holds the line** — so the next write replays afresh
    /// rather than retrying a segment that will never be free (`M6.17`).
    pub(super) async fn journal(&mut self, entry: &MetadataEntry) -> Result<(), CoordinatorError> {
        match self.log.append(core::slice::from_ref(entry)).await {
            Ok(()) => Ok(()),
            Err(error) => {
                if matches!(error, oqueue_core::Error::PreconditionFailed { .. }) {
                    self.replayed_term = None;
                }
                Err(CoordinatorError::Journal(error))
            }
        }
    }

    /// Refuses the acknowledgement of an append that returned after this
    /// loop's lease lapsed (`M6.19`, M6's second closing round).
    ///
    /// ⚠️ **The append may be durable, and it may be lost**: a successor
    /// opens no earlier than the old expiry plus the skew, writes, and
    /// checkpoints, and a checkpoint deletes segment keys — so an append
    /// held past the deadline can land at a deleted key, behind the new base,
    /// where no open reads. One that returned *before* the deadline was
    /// visible to any successor. So the lease is checked again here, and a
    /// lapsed one gets no acknowledgement and no allocator step; the next
    /// term replays afresh.
    pub(super) fn still_leads_after_append(&mut self) -> Result<(), CoordinatorError> {
        if self.leads() {
            return Ok(());
        }
        self.replayed_term = None;
        Err(CoordinatorError::Fenced)
    }

    /// Whether this loop may write *now* — no lease, or one still held.
    pub(super) fn leads(&self) -> bool {
        self.lease.as_ref().is_none_or(|lease| lease.is_held())
    }

    /// Replays the log first if [`Coordinator::open_deferred`] left it to
    /// the loop, awaiting `pause()` between failed attempts, then serves as
    /// [`run`](Self::run) does.
    ///
    /// ⚠️ **The pause comes from the runner** because this crate reads no
    /// clock of its own (`AGENTS.md` non-negotiable 5): the broker passes a
    /// timer, a test passes whatever it wants to step.
    ///
    /// [`Coordinator::open_deferred`]: crate::Coordinator::open_deferred
    pub async fn run_retrying<P, F>(mut self, mut pause: P)
    where
        P: FnMut() -> F,
        F: Future<Output = ()>,
    {
        while !self.try_replay().await {
            pause().await;
        }
        self.run().await;
    }

    /// Replays the log if a deferred open left it pending; whether the loop
    /// may now serve.
    pub(super) async fn try_replay(&mut self) -> bool {
        if !self.pending_replay {
            return true;
        }
        let Ok((allocator, last)) = crate::coordinator::replay(&*self.log, &*self.index).await
        else {
            return false;
        };
        self.allocator = allocator;
        self.last_committed = last;
        self.pending_replay = false;
        self.publish_applied();
        true
    }

    /// Whether this loop may write *now*: replayed, and holding its lease if
    /// it has one.
    ///
    /// ⚠️ **An unreplayed loop never writes**: its allocator is empty, and a
    /// commit served from it would reuse every offset the log already holds.
    pub(super) async fn may_write(&mut self) -> Result<(), CoordinatorError> {
        if !self.try_replay().await {
            return Err(CoordinatorError::Unavailable);
        }
        if !self.leads() {
            return Err(CoordinatorError::Fenced);
        }
        if !self.current_for_term().await {
            return Err(CoordinatorError::Unavailable);
        }
        Ok(())
    }
}
