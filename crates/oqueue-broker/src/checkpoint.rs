//! When the metadata log is checkpointed (`M6.4`, `M6.md` tasks 6 and 7).
//!
//! ⚠️ **On journal bytes, never on the wall clock.** A time-based cadence
//! makes cold-start time load-dependent — worst exactly when the system is
//! busiest — so the trigger is how much has been appended since the last
//! snapshot, and the bound on replay is a bound on bytes (doc 13 §5).
//!
//! ⚠️ **Scheduled on completion plus a pause**, not on a fixed period: the
//! next check waits [`CHECKPOINT_PAUSE`] after the previous checkpoint
//! *finished*, so a slow checkpoint can never be overlapped by the next one
//! (doc 13 §5's continuous-checkpointing lock-up).

use core::time::Duration;
use std::sync::{Arc, Mutex, PoisonError};

use oqueue_core::{ObjectStoreLease, ObjectStoreMetadataLog};

/// The coordinator's current view of the metadata log.
///
/// Swapped each time the coordinator reopens the log under a new lease term
/// (`M6.17`), so the cadence checkpoints the view being appended to, not the
/// one from boot.
pub type CurrentLog = Arc<Mutex<Arc<ObjectStoreMetadataLog>>>;

/// Journal bytes appended since the last snapshot that trigger the next.
///
/// ⚠️ **Doc 13 §10.7's "replay tail in the tens of megabytes"**, the
/// convergent default across `KRaft`, etcd and openraft.
/// ⚠️ **A literal, 32 MiB**, so the pinned value is the bytes that run.
pub const CHECKPOINT_JOURNAL_BYTES: u64 = 33_554_432;

/// How long after a check — or a checkpoint's completion — the next check
/// waits.
///
/// ⚠️ **UNDERIVED**: short enough that the tail rarely overshoots the trigger
/// by more than a few seconds of appends, long enough that an idle log costs
/// nothing but a timer.
pub const CHECKPOINT_PAUSE: Duration = Duration::from_secs(10);

/// Checkpoints the current log whenever its unsnapshotted journal reaches
/// the trigger, while this node holds `lease`, for as long as the task runs.
///
/// ⚠️ **Only the leader checkpoints** (`M6.17`): a node that does not hold
/// the lease has a stale view, and its checkpoint would be refused as not
/// the tail at best.
///
/// ⚠️ **A failed checkpoint is retried at the next check, never fatal**: the
/// log is correct without one, only slower to open.
pub async fn checkpoints(current: CurrentLog, lease: Arc<ObjectStoreLease>) {
    checkpoints_at(
        current,
        Some(lease),
        CHECKPOINT_JOURNAL_BYTES,
        CHECKPOINT_PAUSE,
    )
    .await;
}

pub(crate) async fn checkpoints_at(
    current: CurrentLog,
    lease: Option<Arc<ObjectStoreLease>>,
    trigger: u64,
    pause: Duration,
) {
    loop {
        tokio::time::sleep(pause).await;
        if lease.as_ref().is_some_and(|lease| !lease.is_held()) {
            continue;
        }
        let log = Arc::clone(&current.lock().unwrap_or_else(PoisonError::into_inner));
        if log.unsnapshotted_bytes() >= trigger {
            let _ = log.checkpoint().await;
        }
    }
}

#[cfg(test)]
mod tests;
