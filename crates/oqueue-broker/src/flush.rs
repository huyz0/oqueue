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
use tracing::Instrument;

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
    /// ⚠️ **The first half is `ADR-0005` guarantee 2's own third state**
    /// (`M10.29`): a dropped `put` future is not a resolved `Err`, and the
    /// seam's own contract names it now rather than leaving it something
    /// only this call site had worked out.
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
        let span = crate::telemetry::operation_span("produce", "produce");
        let result = self.flush_inner(bundle).instrument(span.clone()).await;
        span.record(
            "outcome",
            if result.is_ok() { "success" } else { "failure" },
        );
        result
    }

    async fn flush_inner(&self, bundle: BundleBuilder) -> Result<CommitAck, FlushError> {
        let started = tokio::time::Instant::now();
        let sealed = bundle.seal().map_err(FlushError::Store)?;
        // ⚠️ Taken before the payload is moved out, and the reason is not
        // borrow-checking: these are the *only* record of what the object
        // holds, and the coordinator is what turns them into offsets.
        let spans = sealed.spans().to_vec();
        let key = {
            let mut namer = self.namer().lock().unwrap_or_else(PoisonError::into_inner);
            namer.next_key().map_err(FlushError::Store)?
        };
        let put_span = crate::telemetry::dependency_span("object_store", "put", "produce");
        self.store()
            .put(&key, sealed.into_payload(), None)
            .instrument(put_span.clone())
            .await
            .map_err(|error| {
                let latency = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                for span in &spans {
                    self.metrics()
                        .record_write(span.topic(), span.partition(), latency, false);
                }
                put_span.record("outcome", "failure");
                crate::telemetry::dependency_failure("object_store", "put", 0, None, "produce");
                FlushError::Store(error)
            })?;
        put_span.record("outcome", "success");
        let committed_spans = spans.clone();
        self.coordinator()
            .commit(key, spans)
            .await
            .map_err(|error| {
                let latency = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                for span in &committed_spans {
                    self.metrics()
                        .record_write(span.topic(), span.partition(), latency, false);
                }
                crate::telemetry::dependency_failure("coordinator", "commit", 0, None, "produce");
                FlushError::Commit(error)
            })
            .inspect(|_ack| {
                let latency = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
                for span in &committed_spans {
                    self.metrics()
                        .record_write(span.topic(), span.partition(), latency, true);
                }
            })
    }
}
