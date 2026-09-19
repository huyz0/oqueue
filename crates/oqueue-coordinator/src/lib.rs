//! Metadata, offset sequencing, and recovery.
//!
//! Because offsets must be totally ordered per partition and object storage
//! offers no atomic append. Sequencing is the one place that ordering is
//! decided, and M3 is where it is built.
//!
//! # Assign → journal → ack
//!
//! `M3.7`, and `ADR-0020`'s shape. Producers race to accumulate and PUT
//! without coordinating; what order their objects landed in is decided
//! *afterwards*, by the order their metadata records reach one log. So:
//!
//! 1. **Assign** — [`Coordinator::commit`] computes the offsets an object's
//!    spans would occupy, and the version its record would take.
//! 2. **Journal** — that record is appended to the metadata log, durably.
//! 3. **Ack** — only then does the allocator take the position, and only then
//!    does a [`CommitAck`] exist for the caller to acknowledge from.
//!
//! ⚠️ **The order is the correctness argument, not a style.** Assigning after
//! journaling would need a second allocator inside the log; acking before
//! journaling would let a client see an offset that a crash could unmake; and
//! taking the position before the append resolves would leave a gap in a line
//! FR-11 requires to be gap-free whenever the append refuses.
//!
//! # Push for the tail, pull for history
//!
//! `M3.9`, and doc 12 §4.4's already-settled answer (doc 10 #9). The
//! coordinator folds each committed entry into its own
//! [`MaterializedIndex`](oqueue_core::MaterializedIndex) *before* the ack
//! returns, so a producer's own read-your-writes needs no wait at all, then
//! publishes it two ways:
//!
//! - [`Coordinator::watch`] — how far the local index has folded, which a
//!   parked fetch waits on against its own `fetch.max.wait.ms` deadline.
//! - [`Coordinator::subscribe`] — the entries themselves, for a follower
//!   keeping its own materialization. A follower that falls behind is told to
//!   re-bootstrap from the log rather than silently skipping.
//!
//! ⚠️ **The wire-level park is not here.** `M3.9` builds what a fetch waits
//! *on*; `M3.14` is where `oqueue-broker`'s Fetch handler stops ignoring
//! `max_wait_ms` and waits on it, because M2 shipped no park to wire into —
//! `fetch.rs` parses the field and discards it — and this crate has no
//! composer to reach the broker from.
//!
//! ⚠️ **One allocator per metadata shard, not per partition** (`ADR-0020`).
//! A [`CommitVersion`](oqueue_core::CommitVersion) is ordered only within its
//! shard and must never be compared across two.
#![forbid(unsafe_code)]

mod allocator;
mod commit;
mod coordinator;
mod error;
mod serve;
mod standby;
mod subscribe;

pub use allocator::RejectReason;
pub use commit::{Assignment, CommitAck, SpanOutcome, UNASSIGNED_OFFSET};
pub use coordinator::{COMMIT_QUEUE_DEPTH, Coordinator};
pub use error::{CoordinatorError, OpenRejected};
pub use serve::{CoordinatorLoop, REBUILD_PAGE_ENTRIES};
pub use standby::Standby;
pub use subscribe::{DELTA_BUFFER_ENTRIES, DeltaLag, DeltaStream, IndexWatch};
