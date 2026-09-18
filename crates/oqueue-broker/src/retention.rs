//! The task that makes retention happen: FR-33's "including on partitions
//! nobody is writing to" (`M5.91`).
//!
//! ⚠️ **Driven by time, never by a write.** A round runs every
//! [`RETENTION_ROUND_INTERVAL`] whether or not anything was produced, pops
//! what `ExpiryHeap` says has expired, journals each trim through
//! [`Coordinator::trim`], and hands the objects the trim dropped to the
//! `Lifecycle`, whose sweep deletes each once nothing names it and FR-35's
//! delay has passed.
//!
//! ⚠️ **Its own index, folded from the coordinator's delta stream**, the
//! follower shape doc 12 §4.4 describes (`ADR-0024`: the coordinator's index
//! is reachable only through an `IndexReader`, which cannot answer
//! retention's questions). A lagging follower is safe for deletion: keys are
//! never reused (`ADR-0037`), so an entry it has not seen yet can only name an
//! object it was never asked to release.

use core::time::Duration;
use std::sync::Arc;

use oqueue_compact::{DEFAULT_RETENTION_MS, DELETION_DELAY_MS, ExpiryHeap, GcTerms, Lifecycle};
use oqueue_coordinator::{
    Coordinator, CoordinatorError, DeltaLag, DeltaStream, REBUILD_PAGE_ENTRIES,
};
use oqueue_core::{
    Clock, CommitVersion, MaterializedIndex, MetadataEntry, MetadataLog, MetadataRecord, ObjectKey,
    ObjectStore, Offset, PartitionId, Result, TopicId,
};

/// How often a retention round runs.
///
/// ⚠️ **UNDERIVED**, and cheap to run often: a round pops only what is due
/// (`ADR-0036`), so an idle cluster's round touches nothing. Thirty seconds
/// is a delay no retention measured in days can notice.
pub const RETENTION_ROUND_INTERVAL: Duration = Duration::from_secs(30);

/// Retention for one metadata shard, until its coordinator stops.
pub struct Retention {
    coordinator: Coordinator,
    stream: DeltaStream,
    log: Arc<dyn MetadataLog>,
    index: Box<dyn MaterializedIndex>,
    store: Arc<dyn ObjectStore>,
    clock: Arc<dyn Clock>,
    heap: ExpiryHeap,
    lifecycle: Lifecycle,
    /// Whether `index` holds the log. ⚠️ **False after a failed rebuild, and
    /// then nothing is trimmed or swept**: an empty follower names no object,
    /// so a sweep against it would start every released object's clock —
    /// including one another partition still reads (`M5.91`'s review).
    synced: bool,
}

/// ⚠️ Names no partition, for the reason the coordinator's own `Debug` gives.
impl core::fmt::Debug for Retention {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Retention")
            .field("armed", &self.heap.len())
            .field("backlog", &self.lifecycle.backlog())
            .finish_non_exhaustive()
    }
}

impl Retention {
    /// A retention task over `coordinator`'s shard, following it into `index`.
    ///
    /// ⚠️ **Subscribes here**, so everything committed after this call reaches
    /// the follower; what the log held before it is read at the first round.
    ///
    /// # Errors
    ///
    /// [`Error::GcInequalityViolated`](oqueue_core::Error::GcInequalityViolated)
    /// if the configured deletion delay does not exceed FR-35's bound — the
    /// startup check `M5.22` asks for, made with `GcTerms::CONFIGURED`.
    pub fn new(
        coordinator: Coordinator,
        log: Arc<dyn MetadataLog>,
        index: Box<dyn MaterializedIndex>,
        store: Arc<dyn ObjectStore>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        let lifecycle = Lifecycle::new(DELETION_DELAY_MS, GcTerms::CONFIGURED)?;
        let stream = coordinator.subscribe();
        index.clear();
        Ok(Self {
            coordinator,
            stream,
            log,
            index,
            store,
            clock,
            heap: ExpiryHeap::new(DEFAULT_RETENTION_MS),
            lifecycle,
            synced: false,
        })
    }

    /// Folds deltas as they arrive and runs a round every interval, until the
    /// coordinator stops.
    ///
    /// ⚠️ **It holds a coordinator handle**, and the coordinator's loop stops
    /// only once every handle is dropped — so an orderly shutdown aborts this
    /// task first. It returns on its own only when the loop has stopped for
    /// another reason.
    pub async fn run(mut self) {
        self.rebuild().await;
        let mut tick = tokio::time::interval(RETENTION_ROUND_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                delta = self.stream.recv() => match delta {
                    Ok(entry) => self.fold(entry).await,
                    Err(DeltaLag::Lagged { .. }) => self.rebuild().await,
                    Err(DeltaLag::Closed) => return,
                },
                _ = tick.tick() => {
                    if !self.round().await {
                        return;
                    }
                }
            }
        }
    }

    /// Folds one pushed entry, and arms every partition a commit touched.
    async fn fold(&mut self, entry: MetadataEntry) {
        // ⚠️ The overlap a follower strips itself (`Coordinator::subscribe`).
        if self
            .index
            .applied_upto()
            .is_some_and(|applied| entry.version() <= applied)
        {
            return;
        }
        if self.index.apply(core::slice::from_ref(&entry)).is_err() {
            self.rebuild().await;
            return;
        }
        self.arm(&entry);
    }

    fn arm(&mut self, entry: &MetadataEntry) {
        if let MetadataRecord::BatchCommitted { spans, .. } = entry.record() {
            for span in spans {
                self.heap
                    .track(&*self.index, span.topic(), span.partition());
            }
        }
    }

    /// Re-derives the follower and the heap from the log.
    ///
    /// ⚠️ **A failed read leaves the follower unsynced**, and an unsynced
    /// follower trims and deletes nothing; the next round tries again.
    async fn rebuild(&mut self) {
        self.synced = false;
        self.index.clear();
        self.heap = ExpiryHeap::new(DEFAULT_RETENTION_MS);
        let mut from = CommitVersion::ZERO;
        loop {
            let Ok(page) = self.log.read_from(from, REBUILD_PAGE_ENTRIES).await else {
                self.index.clear();
                return;
            };
            let Some(last) = page.last() else {
                self.synced = true;
                return;
            };
            let Ok(next) = last.version().advance(1) else {
                self.index.clear();
                return;
            };
            if self.index.apply(&page).is_err() {
                self.index.clear();
                return;
            }
            for entry in &page {
                self.arm(entry);
            }
            from = next;
        }
    }

    /// One round: journal what has expired, then sweep. False once the
    /// coordinator has stopped.
    async fn round(&mut self) -> bool {
        if !self.synced {
            self.rebuild().await;
            if !self.synced {
                return true;
            }
        }
        let now = self.clock.now();
        // ⚠️ **Backpressure before the heap is popped**: a popped partition
        // that is then not trimmed stays unarmed until its next commit.
        if self.lifecycle.admits() {
            let trims = self.heap.due(&*self.index, now).unwrap_or_default();
            for record in trims {
                let MetadataRecord::Trimmed {
                    topic,
                    partition,
                    start,
                } = record
                else {
                    continue;
                };
                let dropped = self.below(&topic, partition, start);
                match self.coordinator.trim(topic.clone(), partition, start).await {
                    Ok(_) => {
                        for object in dropped {
                            self.lifecycle.release(object);
                        }
                    }
                    Err(CoordinatorError::Unavailable) => return false,
                    // ⚠️ **Re-armed, never dropped**: the heap popped it, and
                    // an idle partition gets no commit to arm it again. A
                    // journal refusal or a trim past an end the follower had
                    // not reached is retried next round; nothing is released.
                    Err(_) => self.heap.track(&*self.index, &topic, partition),
                }
            }
        }
        self.lifecycle.sweep(&*self.index, &*self.store, now).await;
        true
    }

    /// The objects behind every entry a trim to `start` drops.
    ///
    /// ⚠️ **A read failure yields fewer**, and fewer released is a leak, never
    /// an early delete.
    fn below(&self, topic: &TopicId, partition: PartitionId, start: Offset) -> Vec<ObjectKey> {
        let mut dropped = Vec::new();
        let mut cursor = self.index.log_start(topic, partition);
        while cursor < start {
            let Ok(page) = self.index.find_batches(topic, partition, cursor, u64::MAX) else {
                break;
            };
            if page.is_empty() {
                break;
            }
            for batch in &page {
                let Ok(end) = batch.reference().end_offset() else {
                    return dropped;
                };
                if end > start {
                    return dropped;
                }
                dropped.push(batch.reference().object().clone());
                cursor = end;
            }
        }
        dropped
    }
}

#[cfg(test)]
mod tests;
