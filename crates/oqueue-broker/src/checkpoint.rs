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
use std::sync::Arc;

use oqueue_core::ObjectStoreMetadataLog;

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

/// Checkpoints `log` whenever its unsnapshotted journal reaches the trigger,
/// for as long as the task runs.
///
/// ⚠️ **A failed checkpoint is retried at the next check, never fatal**: the
/// log is correct without one, only slower to open. One that fails because
/// this writer is no longer the tail keeps failing, which is the fence
/// working, and costs a GET per pause.
pub async fn checkpoints(log: Arc<ObjectStoreMetadataLog>) {
    checkpoints_at(log, CHECKPOINT_JOURNAL_BYTES, CHECKPOINT_PAUSE).await;
}

pub(crate) async fn checkpoints_at(
    log: Arc<ObjectStoreMetadataLog>,
    trigger: u64,
    pause: Duration,
) {
    loop {
        tokio::time::sleep(pause).await;
        if log.unsnapshotted_bytes() >= trigger {
            let _ = log.checkpoint().await;
        }
    }
}

#[cfg(test)]
mod tests;
