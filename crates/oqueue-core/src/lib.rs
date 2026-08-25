//! Types, IDs, errors, and every trait seam the rest of the workspace is
//! written against.
//!
//! This is the one crate everything depends on, and the one crate that depends
//! on no other crate here — the centre of `architecture.md`'s star. It performs
//! no I/O, reads no clock, and names no socket or object store; anything that
//! needs those is expressed as a trait seam whose implementations live
//! downstream (NFR-51).
//!
//! # What is here so far
//!
//! The four core identifiers ([`TopicId`], [`PartitionId`], [`Offset`],
//! [`ObjectKey`]), [`ObjectStore`]'s own vocabulary ([`ByteRange`],
//! [`ObjectMeta`], [`PreconditionToken`], [`Precondition`]), the [`Error`]
//! enum they can produce, and [`Redacted`] — the wrapper that makes a secret
//! unprintable so FR-44 holds by construction rather than by everyone
//! remembering. All three trait seams — [`Clock`],
//! [`ObjectStore`] and [`KeyProvider`] — are here, each with its fake beside
//! it. ⚠️ Shaped as ADR-0002 decided, which is a rule about the **async**
//! seams: [`ObjectStore`] and [`KeyProvider`] are one `dyn`-compatible trait
//! each, returning a hand-written boxed future. [`Clock`] is **not async and
//! needs no boxing at all** — ADR-0002 says so in as many words, and ADR-0004
//! rejects a per-call allocation on it by name.
//!
//! `M3` adds the coordinator's vocabulary: [`CommitVersion`], the one
//! monotonic scalar `ADR-0020` makes every staleness comparison rest on;
//! [`CoordinatorEpoch`], the fence a reader revalidates against after a
//! failover; [`ReadMode`], which puts the freshness choice at the call site
//! that knows what correctness it needs; and [`MetadataRecord`] with its
//! [`CommittedSpan`], the log entry shaped as an **event** rather than a
//! key-value pair — a delta a snapshot can fold, not a state a compaction
//! would have to arbitrate.
//!
//! # The property every identifier here shares
//!
//! ⚠️ **Each one's invariant is unconstructible-around**, not merely checked at
//! the door. The field is private, the only constructor validates, and there is
//! no setter — so a value that violates the invariant is not a value this crate
//! can hand out. That is what lets everything downstream stop re-checking.
#![forbid(unsafe_code)]

mod byte_range;
mod chunk;
mod clock;
mod commit_version;
mod coordinator_epoch;
mod error;
mod fault;
mod key;
mod key_layout;
mod merge;
mod metadata_record;
mod multipart;
mod object_key;
mod object_meta;
mod offset;
mod op_counts;
mod partition;
mod precondition;
mod rate_governor;
mod read_mode;
mod redacted;
mod retry;
mod store;
#[cfg(test)]
mod test_executor;
mod topic;

pub use byte_range::ByteRange;
pub use chunk::ChunkedObjectStore;
pub use clock::{Clock, FakeClock, Timestamp};
pub use commit_version::CommitVersion;
pub use coordinator_epoch::CoordinatorEpoch;
pub use error::{Error, Result};
pub use fault::{FaultConfig, StormKind};
pub use key::{FakeKeyProvider, KeyId, KeyProvider, WrappedKey};
pub use key_layout::KeyLayout;
pub use merge::MergingObjectStore;
pub use metadata_record::{CommittedSpan, MetadataRecord};
pub use multipart::{MultipartLimits, MultipartSession};
pub use object_key::ObjectKey;
pub use object_meta::{ObjectMeta, PreconditionToken};
pub use offset::Offset;
pub use op_counts::{CountingObjectStore, OpCounts, Operation};
pub use partition::PartitionId;
pub use precondition::Precondition;
pub use rate_governor::{OpClass, RateDecision, RateGovernor, RateLimitPolicy};
pub use read_mode::ReadMode;
pub use redacted::Redacted;
pub use retry::{RetryClass, RetryDecision, RetryPolicy};
pub use store::{BoxFuture, FakeObjectStore, ObjectStore};
pub use topic::TopicId;
