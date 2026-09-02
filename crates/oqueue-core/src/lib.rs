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
//! would have to arbitrate. [`MetadataLog`] is the seam those records are
//! appended to, with [`FakeMetadataLog`] beside it; the durable engine behind
//! it is deliberately still open (`ADR-0020` point 5, doc 10 #12).
//!
//! # The property every identifier here shares
//!
//! ⚠️ **Each one's invariant is unconstructible-around**, not merely checked at
//! the door. The field is private, the only constructor validates, and there is
//! no setter — so a value that violates the invariant is not a value this crate
//! can hand out. That is what lets everything downstream stop re-checking.
#![forbid(unsafe_code)]

mod authz;
mod bundle;
mod bundle_footer;
mod bundle_name;
mod byte_range;
mod chunk;
mod clock;
mod commit_version;
mod coordinator_epoch;
mod error;
mod fault;
mod fault_metadata_log;
mod group_coordinator;
mod group_id;
mod group_protocol;
mod group_state;
mod index_reader;
mod index_state;
mod key;
mod key_layout;
mod materialized_index;
mod member_id;
mod merge;
mod metadata_log;
mod metadata_record;
mod multipart;
mod object_key;
mod object_meta;
mod object_ref;
mod offset;
mod op_counts;
mod partition;
mod precondition;
mod principal;
mod principal_quota;
mod producer_epoch;
mod producer_id;
mod producer_identity;
mod rate_governor;
mod read_mode;
mod redacted;
mod retry;
mod staleness;
mod store;
#[cfg(test)]
mod test_executor;
mod topic;
mod topic_grants;

pub use authz::authorize;
pub use bundle::{BUNDLE_FORMAT_VERSION, BundleBuilder, PushedRecords, Region, RegionAlg, Sealed};
pub use bundle_footer::parse_footer;
pub use bundle_name::BundleNamer;
pub use byte_range::ByteRange;
pub use chunk::ChunkedObjectStore;
pub use clock::{Clock, FakeClock, Timestamp};
pub use commit_version::CommitVersion;
pub use coordinator_epoch::CoordinatorEpoch;
pub use error::{Error, Result};
pub use fault::{FaultConfig, StormKind};
pub use fault_metadata_log::{FaultMetadataLog, LogFaults};
pub use group_coordinator::{FakeGroupCoordinator, GroupCoordinator, GroupRecord};
pub use group_id::GroupId;
pub use group_protocol::{Candidate, Elected, elect};
pub use group_state::{AssignmentEpoch, GenerationId, GroupEvent, GroupState, MemberEpoch};
pub use index_reader::IndexReader;
pub use index_state::{IndexState, MAX_BATCHES_PER_PAGE, TAIL_WINDOW_ENTRIES};
pub use key::{FakeKeyProvider, KeyId, KeyProvider, WrappedKey};
pub use key_layout::KeyLayout;
pub use materialized_index::{FakeMaterializedIndex, MaterializedIndex};
pub use member_id::MemberId;
pub use merge::MergingObjectStore;
pub use metadata_log::{FakeMetadataLog, MetadataEntry, MetadataLog};
pub use metadata_record::{CommittedSpan, MetadataRecord};
pub use multipart::{MultipartLimits, MultipartSession};
pub use object_key::ObjectKey;
pub use object_meta::{ObjectMeta, PreconditionToken};
pub use object_ref::{IndexedBatch, ObjectRef, TailEntry};
pub use offset::Offset;
pub use op_counts::{CountingObjectStore, OpCounts, Operation};
pub use partition::PartitionId;
pub use precondition::Precondition;
pub use principal::Principal;
pub use principal_quota::{InFlight, PrincipalQuota};
pub use producer_epoch::ProducerEpoch;
pub use producer_id::ProducerId;
pub use producer_identity::ProducerIdentity;
pub use rate_governor::{OpClass, RateDecision, RateGovernor, RateLimitPolicy};
pub use read_mode::ReadMode;
pub use redacted::Redacted;
pub use retry::{RetryClass, RetryDecision, RetryPolicy};
pub use staleness::{CacheState, MAX_METADATA_STALENESS_MS, RefreshReason, SessionWatermark};
pub use store::{BoxFuture, FakeObjectStore, ObjectStore};
pub use topic::TopicId;
pub use topic_grants::TopicGrants;
