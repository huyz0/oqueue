//! Bounded, in-process operational measurements.
//!
//! Metrics are deliberately a value rather than a global registry. A
//! composition root can share one handle across the broker's seams, while
//! unit tests can own an isolated instance. Aggregate counters are always
//! available; named partition diagnostics are retained only up to
//! [`MAX_SCOPED_PARTITIONS`]. The cap is part of the contract, not an exporter
//! configuration that can be forgotten.

use crate::{PartitionId, TopicId};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Maximum number of named partition series retained for scoped diagnostics.
///
/// Routine metrics remain aggregate, so a catalog with millions of partitions
/// cannot turn one request into millions of permanent time series. A named
/// diagnostic can still retain a bounded working set for an operator query.
pub const MAX_SCOPED_PARTITIONS: usize = 256;

#[derive(Debug, Default)]
struct Counter {
    count: AtomicU64,
    failures: AtomicU64,
    total_micros: AtomicU64,
}

#[derive(Debug, Default)]
struct State {
    writes: Counter,
    coordinator_ready: AtomicBool,
    coordinator_failures: AtomicU64,
    compaction_backlog: AtomicU64,
    storage_failures: AtomicU64,
    key_domain_failures: AtomicU64,
    encryption_cache_hits: AtomicU64,
    encryption_cache_misses: AtomicU64,
    index_entries: AtomicU64,
    dropped_scoped_samples: AtomicU64,
    partitions: Mutex<BTreeMap<(TopicId, PartitionId), PartitionMetric>>,
}

/// A cheap, cloneable handle to one bounded metrics set.
#[derive(Clone, Debug, Default)]
pub struct OperationalMetrics {
    state: Arc<State>,
}

/// Aggregate write measurements.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteMetrics {
    /// Number of write observations.
    pub count: u64,
    /// Number of writes that failed before completion.
    pub failures: u64,
    /// Sum of observed write latency in microseconds.
    pub total_latency_micros: u64,
}

/// Bounded diagnostic measurements for one named partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionMetric {
    /// The topic this series names.
    pub topic: TopicId,
    /// The partition this series names.
    pub partition: PartitionId,
    /// Latest observed consumer lag in records.
    pub lag: u64,
    /// Number of writes observed for this partition.
    pub write_count: u64,
    /// Sum of write latency in microseconds for this partition.
    pub write_latency_micros: u64,
}

/// A point-in-time copy suitable for an exporter or diagnostic response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationalMetricsSnapshot {
    /// Aggregate write measurements.
    pub writes: WriteMetrics,
    /// Whether coordinator replay has completed successfully.
    pub coordinator_ready: bool,
    /// Number of permanent or failed coordinator replay observations.
    pub coordinator_failures: u64,
    /// Latest observed compaction deletion backlog.
    pub compaction_backlog: u64,
    /// Number of object-storage failures observed by the broker.
    pub storage_failures: u64,
    /// Number of KMS failures observed for customer-key domains.
    pub key_domain_failures: u64,
    /// Read-side DEK cache hits.
    pub encryption_cache_hits: u64,
    /// Read-side or write-side DEK cache misses/rotations.
    pub encryption_cache_misses: u64,
    /// Latest exact number of entries held by the materialized index.
    pub index_entries: u64,
    /// Number of scoped samples rejected after the bounded map filled.
    pub dropped_scoped_samples: u64,
    /// Named partition diagnostics, at most [`MAX_SCOPED_PARTITIONS`].
    pub partitions: Vec<PartitionMetric>,
}

impl OperationalMetrics {
    /// Records one write observation and, when capacity permits, its partition
    /// scoped counterpart.
    pub fn record_write(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        latency_micros: u64,
        success: bool,
    ) {
        self.state.writes.count.fetch_add(1, Ordering::Relaxed);
        self.state
            .writes
            .total_micros
            .fetch_add(latency_micros, Ordering::Relaxed);
        if !success {
            self.state.writes.failures.fetch_add(1, Ordering::Relaxed);
        }
        let mut partitions = self
            .state
            .partitions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let key = (topic.clone(), partition);
        if let Some(metric) = partitions.get_mut(&key) {
            metric.write_count = metric.write_count.saturating_add(1);
            metric.write_latency_micros =
                metric.write_latency_micros.saturating_add(latency_micros);
        } else if partitions.len() < MAX_SCOPED_PARTITIONS {
            partitions.insert(
                key,
                PartitionMetric {
                    topic: topic.clone(),
                    partition,
                    lag: 0,
                    write_count: 1,
                    write_latency_micros: latency_micros,
                },
            );
        } else {
            self.state
                .dropped_scoped_samples
                .fetch_add(1, Ordering::Relaxed);
        }
        drop(partitions);
    }

    /// Records the latest observed lag for one partition.
    pub fn record_lag(&self, topic: &TopicId, partition: PartitionId, lag: u64) {
        let mut partitions = self
            .state
            .partitions
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let key = (topic.clone(), partition);
        if let Some(metric) = partitions.get_mut(&key) {
            metric.lag = lag;
        } else if partitions.len() < MAX_SCOPED_PARTITIONS {
            partitions.insert(
                key,
                PartitionMetric {
                    topic: topic.clone(),
                    partition,
                    lag,
                    write_count: 0,
                    write_latency_micros: 0,
                },
            );
        } else {
            self.state
                .dropped_scoped_samples
                .fetch_add(1, Ordering::Relaxed);
        }
        drop(partitions);
    }

    /// Publishes the coordinator's latest readiness observation.
    pub fn record_coordinator_ready(&self, ready: bool) {
        self.state.coordinator_ready.store(ready, Ordering::Relaxed);
        if !ready {
            self.state
                .coordinator_failures
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records an object-storage failure for health classification.
    pub fn record_storage_failure(&self) {
        self.state.storage_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a KMS failure without naming the affected customer-key domain.
    pub fn record_key_domain_failure(&self) {
        self.state
            .key_domain_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Publishes the latest compaction deletion backlog.
    pub fn record_compaction_backlog(&self, backlog: usize) {
        self.state
            .compaction_backlog
            .store(backlog as u64, Ordering::Relaxed);
    }

    /// Records a DEK cache hit or miss/rotation.
    pub fn record_dek_cache_hit(&self) {
        self.state
            .encryption_cache_hits
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records a DEK cache miss or rotation.
    pub fn record_dek_cache_miss(&self) {
        self.state
            .encryption_cache_misses
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Publishes the exact materialized-index entry count.
    pub fn record_index_entries(&self, entries: usize) {
        self.state
            .index_entries
            .store(entries as u64, Ordering::Relaxed);
    }

    /// Takes a consistent-enough point-in-time copy for diagnostics. Atomic
    /// fields may be from adjacent instants; metrics do not claim a snapshot
    /// transaction, and no correctness decision may use this value.
    #[must_use]
    pub fn snapshot(&self) -> OperationalMetricsSnapshot {
        let partitions = self
            .state
            .partitions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        OperationalMetricsSnapshot {
            writes: WriteMetrics {
                count: self.state.writes.count.load(Ordering::Relaxed),
                failures: self.state.writes.failures.load(Ordering::Relaxed),
                total_latency_micros: self.state.writes.total_micros.load(Ordering::Relaxed),
            },
            coordinator_ready: self.state.coordinator_ready.load(Ordering::Relaxed),
            coordinator_failures: self.state.coordinator_failures.load(Ordering::Relaxed),
            compaction_backlog: self.state.compaction_backlog.load(Ordering::Relaxed),
            storage_failures: self.state.storage_failures.load(Ordering::Relaxed),
            key_domain_failures: self.state.key_domain_failures.load(Ordering::Relaxed),
            encryption_cache_hits: self.state.encryption_cache_hits.load(Ordering::Relaxed),
            encryption_cache_misses: self.state.encryption_cache_misses.load(Ordering::Relaxed),
            index_entries: self.state.index_entries.load(Ordering::Relaxed),
            dropped_scoped_samples: self.state.dropped_scoped_samples.load(Ordering::Relaxed),
            partitions,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::expect_used,
    clippy::range_plus_one
)]
mod tests {
    use super::*;

    fn topic(name: &str) -> TopicId {
        TopicId::new(name).expect("test topic is non-empty")
    }

    fn partition(index: i32) -> PartitionId {
        PartitionId::new(index).expect("test partition is non-negative")
    }

    #[test]
    fn aggregates_required_signals_and_keeps_scoped_series_bounded() {
        let metrics = OperationalMetrics::default();
        let topic = topic("orders");
        let first_partition = partition(3);
        metrics.record_write(&topic, first_partition, 17, true);
        metrics.record_write(&topic, first_partition, 23, true);
        metrics.record_write(&topic, first_partition, 29, false);
        metrics.record_lag(&topic, first_partition, 9);
        metrics.record_coordinator_ready(true);
        metrics.record_storage_failure();
        metrics.record_key_domain_failure();
        metrics.record_compaction_backlog(12);
        metrics.record_dek_cache_hit();
        metrics.record_dek_cache_miss();
        metrics.record_index_entries(44);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.writes.count, 3);
        assert_eq!(snapshot.writes.failures, 1);
        assert_eq!(snapshot.writes.total_latency_micros, 69);
        assert!(snapshot.coordinator_ready);
        assert_eq!(snapshot.storage_failures, 1);
        assert_eq!(snapshot.key_domain_failures, 1);
        assert_eq!(snapshot.compaction_backlog, 12);
        assert_eq!(snapshot.encryption_cache_hits, 1);
        assert_eq!(snapshot.encryption_cache_misses, 1);
        assert_eq!(snapshot.index_entries, 44);
        assert_eq!(snapshot.partitions.len(), 1);
        assert_eq!(snapshot.partitions[0].lag, 9);

        let write_metrics = OperationalMetrics::default();
        for index in 0..=(MAX_SCOPED_PARTITIONS as i32) {
            write_metrics.record_write(&topic, partition(index), 1, true);
        }
        let bounded_writes = write_metrics.snapshot();
        assert_eq!(bounded_writes.partitions.len(), MAX_SCOPED_PARTITIONS);
        assert_eq!(
            bounded_writes.dropped_scoped_samples, 1,
            "the first sample past capacity is dropped"
        );

        for index in 0..(MAX_SCOPED_PARTITIONS as i32 + 1) {
            metrics.record_lag(&topic, partition(index), index as u64);
        }
        let bounded = metrics.snapshot();
        assert_eq!(bounded.partitions.len(), MAX_SCOPED_PARTITIONS);
        assert!(bounded.dropped_scoped_samples > 0);
    }

    #[test]
    fn clones_share_the_same_measurements() {
        let metrics = OperationalMetrics::default();
        let clone = metrics.clone();
        clone.record_coordinator_ready(false);
        assert_eq!(metrics.snapshot().coordinator_failures, 1);
        assert!(!metrics.snapshot().coordinator_ready);
    }
}
