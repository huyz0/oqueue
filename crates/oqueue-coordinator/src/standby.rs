//! A hot standby: a coordinator-in-waiting that folds the log as it grows,
//! so that failover is a promotion and not a rebuild (`M6.8`, `M6.md` task
//! 10, doc 13 §2(b)).
//!
//! ⚠️ **What promotion saves is the fold, not the wait for the lease.** A
//! standby still waits out its predecessor's lease before it may lead (`M6.7`);
//! what it does not do after that is read and fold the whole log, because it
//! already has — only the tail since its last catch-up is left.
//!
//! ⚠️ **It folds with the live path's own arithmetic** — `Allocator::replay`
//! and `MaterializedIndex::apply` — so a promoted standby's line is the line a
//! cold replay would have produced.

use std::sync::Arc;

use oqueue_core::{
    Clock, CommitVersion, CoordinatorEpoch, IndexReader, MaterializedIndex, MetadataLog,
};

use crate::Coordinator;
use crate::allocator::Allocator;
use crate::error::CoordinatorError;
use crate::serve::{CoordinatorLoop, REBUILD_PAGE_ENTRIES};

/// A shard's line, folded so far, waiting to be promoted.
pub struct Standby {
    allocator: Allocator,
    index: Box<dyn MaterializedIndex>,
    last: Option<CommitVersion>,
}

/// ⚠️ Names how far it has folded, never what.
impl core::fmt::Debug for Standby {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Standby")
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

impl Standby {
    /// A standby that has folded nothing, over an index it will own.
    #[must_use]
    pub fn new(index: Box<dyn MaterializedIndex>) -> Self {
        index.clear();
        Self {
            allocator: Allocator::new(),
            index,
            last: None,
        }
    }

    /// The last version folded, if any.
    #[must_use]
    pub const fn folded_upto(&self) -> Option<CommitVersion> {
        self.last
    }

    /// Folds every entry `log` holds past what this standby has folded;
    /// returns how many.
    ///
    /// ⚠️ **`log` must be a current view of the shard's log** — for the
    /// object-store log, one opened afresh, since an open log does not see
    /// another process's appends.
    ///
    /// # Errors
    ///
    /// [`CoordinatorError::Journal`] if the log cannot be read, and
    /// [`CoordinatorError::Unreplayable`] for an entry the fold refuses — after
    /// which this standby must be rebuilt, not caught up again.
    pub async fn catch_up(&mut self, log: &dyn MetadataLog) -> Result<usize, CoordinatorError> {
        let mut folded = 0;
        loop {
            let from = match self.last {
                None => CommitVersion::ZERO,
                Some(last) => last
                    .advance(1)
                    .map_err(|source| CoordinatorError::Unreplayable {
                        version: last.get(),
                        source,
                    })?,
            };
            let page = log
                .read_from(from, REBUILD_PAGE_ENTRIES)
                .await
                .map_err(CoordinatorError::Journal)?;
            let Some(tail) = page.last() else {
                return Ok(folded);
            };
            let unreplayable = |version: CommitVersion| {
                move |source| CoordinatorError::Unreplayable {
                    version: version.get(),
                    source,
                }
            };
            for entry in &page {
                self.allocator
                    .replay(entry)
                    .map_err(unreplayable(entry.version()))?;
            }
            self.index
                .apply(&page)
                .map_err(unreplayable(tail.version()))?;
            self.last = Some(tail.version());
            folded += page.len();
        }
    }

    /// Becomes the shard's coordinator: one last catch-up over `log`, then a
    /// coordinator serving from the line already folded.
    ///
    /// # Errors
    ///
    /// Whatever the final [`catch_up`](Self::catch_up) refuses.
    pub async fn promote(
        mut self,
        log: Arc<dyn MetadataLog>,
        epoch: CoordinatorEpoch,
        clock: Arc<dyn Clock>,
    ) -> Result<(Coordinator, CoordinatorLoop, IndexReader), CoordinatorError> {
        self.catch_up(&*log).await?;
        Ok(Coordinator::assemble(
            log,
            self.index,
            epoch,
            clock,
            (self.allocator, self.last, false),
        ))
    }
}
