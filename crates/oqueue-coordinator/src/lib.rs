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
//! ⚠️ **One allocator per metadata shard, not per partition** (`ADR-0020`).
//! A [`CommitVersion`](oqueue_core::CommitVersion) is ordered only within its
//! shard and must never be compared across two.
#![forbid(unsafe_code)]

mod allocator;
mod commit;
mod coordinator;
mod error;

pub use commit::{Assignment, CommitAck, UNASSIGNED_OFFSET};
pub use coordinator::{COMMIT_QUEUE_DEPTH, Coordinator, CoordinatorLoop};
pub use error::CoordinatorError;
