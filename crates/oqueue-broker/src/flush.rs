//! The write path: seal one bundle, PUT it once, commit its spans.
//!
//! ⚠️ **Its own module because the order is the correctness argument**, and
//! `ADR-0020`'s assign→journal→ack only holds if the object is durable before
//! the record naming it is. A commit that preceded the PUT would journal a
//! position for bytes that may never exist, and FR-10's "acknowledged means
//! durable" would be a claim about intent.

use crate::cluster::{Cluster, FlushError};
use oqueue_coordinator::CommitAck;
use oqueue_core::BundleBuilder;
use std::sync::PoisonError;

impl Cluster {
    /// Seals `bundle`, writes it as one object, and commits the spans it
    /// sealed.
    ///
    /// ⚠️ **One PUT, whatever `bundle` spans** — FR-32, and the wire-level
    /// half `M3.13` established as a property of the format. The store call
    /// below is the only one on this path.
    ///
    /// ⚠️ **The `put` carries no [`Precondition`]** — `ADR-0026`, resting
    /// entirely on the key being one no other live process writes, which is
    /// [`WriterId`](crate::WriterId)'s whole job. A retried flush rewrites its
    /// own key with its own bytes.
    ///
    /// ⚠️ **Not cancellation-safe** (`async-concurrency.md` rule 10), and it
    /// inherits that from [`Coordinator::commit`]. A caller that drops this
    /// future after the PUT has landed leaves an object nothing references —
    /// garbage, not corruption, and `M5`'s reaper is what collects it. A
    /// caller that drops it after the commit was queued may see no ack for a
    /// commit that happened, which is the case a client retry re-produces and
    /// a duplicate results from; idempotent produce (`M11`) is what closes it.
    ///
    /// [`Precondition`]: oqueue_core::Precondition
    /// [`Coordinator::commit`]: oqueue_coordinator::Coordinator::commit
    ///
    /// # Errors
    ///
    /// [`FlushError::Store`] if the bundle cannot be sealed, named, or
    /// written; [`FlushError::Commit`] if it was written and its position
    /// could not be journalled.
    pub async fn flush(&self, bundle: BundleBuilder) -> Result<CommitAck, FlushError> {
        let sealed = bundle.seal().map_err(FlushError::Store)?;
        // ⚠️ Taken before the payload is moved out, and the reason is not
        // borrow-checking: these are the *only* record of what the object
        // holds, and the coordinator is what turns them into offsets.
        let spans = sealed.spans().to_vec();
        let key = {
            let mut namer = self.namer().lock().unwrap_or_else(PoisonError::into_inner);
            namer.next_key().map_err(FlushError::Store)?
        };
        self.store()
            .put(&key, sealed.into_payload(), None)
            .await
            .map_err(FlushError::Store)?;
        self.coordinator()
            .commit(key, spans)
            .await
            .map_err(FlushError::Commit)
    }
}
