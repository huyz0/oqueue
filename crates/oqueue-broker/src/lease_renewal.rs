//! Keeping a coordinator's lease, for as long as it is its to keep (`M6.7`).
//!
//! ⚠️ **Renewal is what polls** (doc 13 §10.10): each one first looks for a
//! successor's term, so a holder learns it was superseded within one period
//! rather than at its deadline — and it never relies on being told.

use core::time::Duration;
use std::sync::Arc;

use oqueue_core::{LEASE_RENEW_MS, ObjectStoreLease};

/// Renews `lease` every [`LEASE_RENEW_MS`] until a successor has taken it.
///
/// ⚠️ **A renewal that errors is retried next period, not fatal**: the lease
/// lapses on its own clock if they keep failing, which is the fence, and the
/// coordinator's loop stops writing at that moment whatever this task does.
pub async fn renew_lease(lease: Arc<ObjectStoreLease>) {
    let period = Duration::from_millis(LEASE_RENEW_MS.unsigned_abs());
    loop {
        tokio::time::sleep(period).await;
        if matches!(lease.renew().await, Ok(false)) {
            return;
        }
    }
}

#[cfg(test)]
mod tests;
