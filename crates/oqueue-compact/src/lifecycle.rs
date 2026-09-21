//! An object's lifecycle after the index stops naming it: live → logically
//! deleted → deletion queue → gone (`M5.21`).
//!
//! ⚠️ **Zero references is the precondition, not the permission** (`ADR-0045`).
//! A reader that resolved a reference just before the index dropped it can
//! still be fetching, so an object waits out a delay counted from the first
//! moment this lifecycle *saw* its count at zero — never from when it was
//! released, which may be long before its last slice died. That delay is
//! FR-35's inequality, whose terms `gc.rs` fixes and `new` checks.
//!
//! ⚠️ **Bounded twice, because a cap alone is not the answer.** A per-round
//! cap bounds what one sweep issues; it does not bound the queue, which grows
//! for as long as creation outruns deletion — a documented pathology in a
//! peer system. [`Lifecycle::admits`] is the backpressure: the caller stops
//! creating garbage (a compaction round, a retention round) while the backlog
//! is at [`DELETION_BACKLOG_KEYS`].
//!
//! ⚠️ **"Will this key ever delete" is answered by repetition** (`ADR-0030`).
//! A per-key refusal from the store is `Transient` and says nothing about why,
//! so a key refused in [`QUARANTINE_AFTER_REFUSALS`] consecutive sweeps is
//! quarantined and reported rather than retried forever.

use std::collections::{HashMap, HashSet, VecDeque};

use oqueue_core::{
    MaterializedIndex, ObjectKey, ObjectStore, OperationalMetrics, Result, Timestamp,
};

use crate::GcTerms;

/// The most keys one sweep asks the store to delete.
///
/// ⚠️ **S3's own `DeleteObjects` ceiling**, 1,000 keys a request, so one sweep
/// is at most one batch request when nothing is refused.
pub const DELETE_BATCH_KEYS: usize = 1_000;

/// The backlog — logically deleted plus queued — above which the lifecycle
/// asks its callers to stop creating garbage.
///
/// ⚠️ **UNDERIVED**: a hundred full sweeps' worth, so a backlog this deep is
/// one the deleter cannot clear at its cap in any reasonable interval.
pub const DELETION_BACKLOG_KEYS: usize = 100_000;

/// Consecutive sweeps a key may be refused in before it is quarantined.
///
/// ⚠️ **UNDERIVED**: enough to ride out a throttling burst, few enough that a
/// key the store will never delete is reported within minutes, not never.
pub const QUARANTINE_AFTER_REFUSALS: u32 = 5;

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Keys the store confirmed deleted.
    pub deleted: usize,
    /// Keys the store refused this sweep, and which stay queued.
    pub refused: usize,
    /// Keys this sweep quarantined: refused too often to keep retrying.
    pub quarantined: Vec<ObjectKey>,
}

/// The objects released from the index, until each is gone.
#[derive(Debug)]
pub struct Lifecycle {
    delay_ms: i64,
    /// Logically deleted: released, waiting for zero references and the delay.
    /// `None` is "not yet seen at zero", so the clock has not started.
    logical: HashMap<ObjectKey, Option<i64>>,
    /// Due for deletion, in promotion order.
    queue: VecDeque<ObjectKey>,
    /// Every key in `queue` or `quarantined`, so a release is one lookup.
    leaving: HashSet<ObjectKey>,
    /// Consecutive refusals per queued key.
    refusals: HashMap<ObjectKey, u32>,
    quarantined: Vec<ObjectKey>,
    metrics: OperationalMetrics,
}

impl Lifecycle {
    /// A lifecycle that waits `delay_ms` after an object's count reaches zero.
    ///
    /// ⚠️ **The inequality is checked here, at startup** (`M5.22`): a delay
    /// that does not exceed `terms`' sum is refused rather than run.
    ///
    /// # Errors
    ///
    /// [`Error::GcInequalityViolated`](oqueue_core::Error::GcInequalityViolated)
    /// when `delay_ms` does not exceed `terms.bound()`.
    pub fn new(delay_ms: i64, terms: GcTerms) -> Result<Self> {
        terms.check(delay_ms)?;
        Ok(Self {
            delay_ms,
            logical: HashMap::new(),
            queue: VecDeque::new(),
            leaving: HashSet::new(),
            refusals: HashMap::new(),
            quarantined: Vec::new(),
            metrics: OperationalMetrics::default(),
        })
    }

    /// Shares an operational metrics set with the composition root.
    #[must_use]
    pub fn with_metrics(mut self, metrics: OperationalMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// The metrics set receiving backlog observations.
    #[must_use]
    pub const fn metrics(&self) -> &OperationalMetrics {
        &self.metrics
    }

    /// Records that the index dropped a reference to `object`.
    ///
    /// ⚠️ **Starts no clock.** The object may still be named by another
    /// partition's slice, and a release time would then predate its last
    /// slice's death — a delay counted from it could expire while a reader of
    /// that slice is still fetching. The clock starts at the first sweep that
    /// sees the count at zero, which is never earlier than the drop.
    /// ⚠️ A key already on its way out is left where it is, so a replayed
    /// release neither restarts its clock nor queues it twice.
    pub fn release(&mut self, object: ObjectKey) {
        if self.leaving.contains(&object) {
            return;
        }
        self.logical.entry(object).or_insert(None);
        self.metrics.record_compaction_backlog(self.backlog());
    }

    /// Whether the caller may create more garbage: false while the backlog is
    /// at [`DELETION_BACKLOG_KEYS`].
    #[must_use]
    pub fn admits(&self) -> bool {
        self.backlog() < DELETION_BACKLOG_KEYS
    }

    /// Logically deleted plus queued keys.
    #[must_use]
    pub fn backlog(&self) -> usize {
        self.logical.len() + self.queue.len()
    }

    /// Every key quarantined so far.
    #[must_use]
    pub fn quarantined(&self) -> &[ObjectKey] {
        &self.quarantined
    }

    /// Promotes what is due, then deletes up to [`DELETE_BATCH_KEYS`] of it.
    ///
    /// ⚠️ **Idempotent**: a deleted key leaves the queue, so a sweep replayed
    /// over the same state deletes nothing twice, and the store's delete is
    /// idempotent per key besides.
    ///
    /// ⚠️ **Infallible by design**: a refusal is counted against its keys,
    /// never propagated, because one bad key must not stall every other.
    pub async fn sweep<I, S>(&mut self, index: &I, store: &S, now: Timestamp) -> SweepReport
    where
        I: MaterializedIndex + ?Sized,
        S: ObjectStore + ?Sized,
    {
        self.promote(index, now.as_millis());
        let take = self.queue.len().min(DELETE_BATCH_KEYS);
        let batch: Vec<ObjectKey> = self.queue.drain(..take).collect();
        let mut report = SweepReport::default();
        if batch.is_empty() {
            self.metrics.record_compaction_backlog(self.backlog());
            return report;
        }
        if store.delete(&batch).await.is_ok() {
            report.deleted = batch.len();
            for key in &batch {
                self.refusals.remove(key);
                self.leaving.remove(key);
            }
            return report;
        }
        // ⚠️ The batch's error names no key, so each is asked alone: a
        // refusal costs requests only on the sweep that met it.
        for key in batch {
            if store.delete(core::slice::from_ref(&key)).await.is_ok() {
                self.refusals.remove(&key);
                self.leaving.remove(&key);
                report.deleted += 1;
                continue;
            }
            report.refused += 1;
            let refused = self.refusals.entry(key.clone()).or_insert(0);
            *refused += 1;
            if *refused >= QUARANTINE_AFTER_REFUSALS {
                self.refusals.remove(&key);
                self.quarantined.push(key.clone());
                report.quarantined.push(key);
            } else {
                self.queue.push_back(key);
            }
        }
        self.metrics.record_compaction_backlog(self.backlog());
        report
    }

    /// Moves every logically deleted key whose count has sat at zero for the
    /// delay onto the queue.
    fn promote<I>(&mut self, index: &I, now: i64)
    where
        I: MaterializedIndex + ?Sized,
    {
        let delay = self.delay_ms;
        let mut due = Vec::new();
        for (key, zero_since) in &mut self.logical {
            if index.references(key) > 0 {
                // ⚠️ Still named — another partition's slice, or a manifest.
                // The clock restarts only once a sweep sees it at zero.
                *zero_since = None;
                continue;
            }
            match *zero_since {
                None => *zero_since = Some(now),
                Some(since) if now.saturating_sub(since) >= delay => due.push(key.clone()),
                Some(_) => {}
            }
        }
        // ⚠️ Oldest-released first would need the times kept; key order is
        // deterministic, which is what a replayed sweep needs.
        due.sort();
        for key in due {
            self.logical.remove(&key);
            self.leaving.insert(key.clone());
            self.queue.push_back(key);
        }
    }
}
