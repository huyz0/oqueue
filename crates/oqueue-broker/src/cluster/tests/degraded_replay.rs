//! The group replay under a store that recovers partway (`M6.10`'s review).

use super::cluster_still_loading_over;
use oqueue_core::{
    BoxFuture, CommitVersion, Error, FakeGroupMetadataLog, GroupId, GroupMetadataEntry,
    GroupMetadataLog, GroupMetadataRecord, Result, TopicId,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A group log whose first read fails transiently and every later one works —
/// a store that comes back between the replay's two halves.
#[derive(Debug)]
struct FailsOnce {
    inner: FakeGroupMetadataLog,
    failed: AtomicBool,
}

impl GroupMetadataLog for FailsOnce {
    fn append<'a>(&'a self, entries: &'a [GroupMetadataEntry]) -> BoxFuture<'a, Result<()>> {
        self.inner.append(entries)
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<GroupMetadataEntry>>> {
        if !self.failed.swap(true, Ordering::SeqCst) {
            return Box::pin(async { Err(Error::Transient) });
        }
        self.inner.read_from(start, max_entries)
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        self.inner.last_version()
    }
}

/// ⚠️ **Readiness waits for both halves** — the offsets half's read fails,
/// the transitions half's succeeds a moment later, and the gate must still
/// stay shut until the offsets have replayed too; opening early would answer
/// `OffsetFetch` from an empty table and send consumers to
/// `auto.offset.reset`.
#[tokio::test(start_paused = true)]
async fn the_gate_waits_for_both_halves_when_the_store_recovers_between_them() {
    let group = GroupId::new("g").expect("a valid group");
    let topic = TopicId::new("orders").expect("a valid topic");
    let log = FailsOnce {
        inner: FakeGroupMetadataLog::new(),
        failed: AtomicBool::new(false),
    };
    log.inner
        .append(&[GroupMetadataEntry::new(
            CommitVersion::ZERO,
            GroupMetadataRecord::OffsetCommitted {
                group: group.clone(),
                topic: topic.clone(),
                partition: 0,
                offset: 42,
            },
        )])
        .await
        .expect("appends");
    let cluster = cluster_still_loading_over(Arc::new(log)).await;
    tokio::time::sleep(crate::DEGRADED_RETRY * 2).await;
    cluster.wait_until_replayed().await;
    assert_eq!(
        cluster.committed_offsets().get(&group, &topic, 0),
        Some(42),
        "the gate opened only once the committed offsets had replayed"
    );
}
