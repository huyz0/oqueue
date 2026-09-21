//! The cluster the API handlers serve against: a topic registry, a
//! coordinator, an index and an object store.
//!
//! ⚠️ **This replaces `M2`'s `StubCluster`, and only half of what that held
//! was a stub.** The topic *registry* — names, ids, partition counts,
//! create-on-produce — is read through `TopicCatalog` since `M7.2`
//! (`cluster/topics.rs`), and a `CreateTopics` API with a creation policy is
//! later.
//! What was a stub is the *log*, and that half is gone: a produce now seals a
//! bundle, PUTs it once, and commits its spans to the coordinator, and a fetch
//! resolves offset→object through the index instead of replaying every batch
//! it ever held.
//!
//! # ⚠️ Offsets are stamped on the way **out**, not on the way in
//!
//! `M2` rewrote a batch's base offset before storing it, because the stub
//! assigned the offset first. `ADR-0020` inverts that: the object is written
//! *before* anyone knows what order it landed in, and offsets are assigned
//! when the metadata record commits — which is after the PUT, because the
//! record names the object. So the bytes in object storage carry whatever base
//! offset the producer sent, and [`Cluster::read`] stamps the real one in as
//! it serves them.
//!
//! ⚠️ **That costs nothing and breaks no checksum**: the base offset is the
//! first twelve bytes of a record batch and the CRC covers only what follows
//! (doc 18 §4.4), which is the property `M2.23`'s in-place rewrite already
//! rested on — the same rewrite, moved to the other end.
//!
//! # ⚠️ One flush, one PUT
//!
//! FR-32, and the wire-level half `M3.13` could not reach for want of a
//! composer. Every partition in one `Produce` request goes into the
//! [`BundleBuilder`](oqueue_core::BundleBuilder) for its key domain, so a
//! default-only request spanning N topics costs **one** PUT and one metadata
//! record. A request spanning domains gets one flush, PUT and metadata record
//! per domain, which is the segregation required by FR-42. Doc 12 prices a
//! PUT far above the bytes in it, so this ratio is the cost model.

use crate::join_group::GroupJoins;
use crate::writer_id::WriterId;
use oqueue_coordinator::{Coordinator, CoordinatorError};
use oqueue_core::{
    BundleNamer, CacheState, CoordinatorEpoch, Error, FakeTopicCatalog, GroupCoordinator,
    GroupMetadataLog, IndexReader, ObjectStore, Offset, PartitionId, TopicCatalog, TopicId,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub use crate::region_sealer::{RegionSealer, RejectingRegionSealer, SealedRegionOwned};

type CreatorTopicsLoad = Arc<tokio::sync::OnceCell<Result<Vec<TopicId>, ()>>>;

/// One node's view of the cluster.
///
/// ⚠️ **Node identity is decided, never inferred** — doc 02 §7.2, unchanged
/// from `M2`: clients treat node id *and* hostname as identity, so a fleet of
/// interchangeable processes needs a deliberate scheme. A single-node broker
/// has a single honest identity; the fleet answer belongs to the milestone
/// that has a fleet.
#[derive(Debug)]
pub struct Cluster {
    /// This broker's node id — 0, the only node.
    pub node_id: i32,
    /// The advertised host, verbatim from the composition root.
    pub host: String,
    /// The advertised port.
    pub port: i32,
    /// What topics exist (`ADR-0049` point 2) — asked on a cache miss.
    catalog: Arc<dyn TopicCatalog>,
    // ⚠️ A std `Mutex`, deliberately (`async-concurrency.md` rules 6 and 8):
    // every critical section below is a map touch or a counter bump with no
    // `.await` inside, and the lock protects data, never control flow.
    // ⚠️ Only topics this node has served — never filled with the catalog.
    topics: Mutex<topics::TopicCache>,
    coordinator: Coordinator,
    index: IndexReader,
    store: Arc<dyn ObjectStore>,
    region_sealer: Arc<dyn RegionSealer>,
    namer: Mutex<BundleNamer>,
    reaped_reads: AtomicU64,
    /// How many catalog entries [`Cluster::partition_count`] and
    /// [`Cluster::topic_names`] have together touched — `M9.17`'s own
    /// cost-test proxy for NFR-12 (`docs/internal/standards/testing.md` rule
    /// 11 forbids asserting on wall-clock duration, so this is what a test
    /// asserting "CPU independent of catalog size" actually counts): one per
    /// `partition_count` call, and one per name a `topic_names` call returns
    /// — bounded by its limit since `M7.4`, never by the catalog. ⚠️ It
    /// counts names, not catalog calls; the scale tests count those.
    topic_lookups: AtomicU64,
    /// One cancellation-safe, single-flight durable creator-index load per
    /// authenticated principal. The result is shared by every dispatcher.
    creator_topic_loads: Mutex<HashMap<oqueue_core::Principal, CreatorTopicsLoad>>,
    /// The consumer-group state machine's own seam (`M4.2`, `ADR-0034`) —
    /// `ADR-0033`'s "every group resolves to this node" made real: one
    /// coordinator instance, shared by every connection's `Dispatcher` the
    /// same way `coordinator`/`index` above already are, since two members
    /// of the same group arrive on two different connections and must see
    /// the same group state.
    group_coordinator: Arc<dyn GroupCoordinator>,
    /// `M4.7`'s own join-round bookkeeping — who is mid-`JoinGroup` for
    /// each group, never durable and never part of `group_coordinator`'s
    /// own sans-I/O contract (`round`'s own module doc). Starts empty by
    /// construction; nothing a composer configures.
    group_joins: GroupJoins,
    /// `M4.8`'s own sync barrier — who is mid-`SyncGroup` for each group,
    /// and the assignment map the leader's own submission fills in.
    /// Starts empty by construction, `group_joins`'s own shape.
    sync_groups: crate::sync_group::SyncGroups,
    /// `M4.9`'s own session-timeout tracking — every group's own tracked
    /// membership, and when each member's own silence would evict it.
    /// Starts empty by construction, `group_joins`'s own shape.
    heartbeats: crate::heartbeat::Heartbeats,
    /// `M4.12`'s own committed-offset bookkeeping — durable since `M4.14`
    /// (`ADR-0035`). ⚠️ **`Arc`, since `M4.15a`**: [`Cluster::new`] no
    /// longer awaits the full replay before returning (`replay_gate`'s own
    /// doc) — it spawns a background task that shares this same instance
    /// to run [`crate::offset_commit::CommittedOffsets::replay`], which
    /// needs to outlive `new`'s own call frame.
    committed_offsets: Arc<crate::offset_commit::CommittedOffsets>,
    /// Whether this node has finished the replay above — `M4.15a`,
    /// `crate::fencing::NodeReadiness::load_in_progress`'s real signal.
    /// `Arc` for the same reason `committed_offsets` is: the background
    /// replay task flips it when done.
    replay_gate: Arc<crate::replay_gate::ReplayGate>,
    /// The background replay task's own handle — `async-concurrency.md`
    /// rule 13's own "an owner that can observe completion and panic,"
    /// inspected by [`Cluster::wait_until_replayed`]. Since `M4.15c` this
    /// is also the group-transitions actor's own task (`cluster/replay.rs`'s
    /// own module doc). `std::sync::Mutex`, not held across an `.await`.
    replay_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The single-writer group-transition seam (`M4.15c`) —
    /// `cluster/replay.rs`'s own `group_transitions()` accessor.
    group_transitions: crate::group_transitions::GroupTransitions,
}

/// A coordinator and the reader over the index it folds into.
///
/// ⚠️ **A pair because they are only meaningful as one.** Both come out of a
/// single `Coordinator::open`, and a `Cluster` handed a reader belonging to a
/// *different* coordinator would answer fetches from one line's index while
/// committing to another's — offsets that look ordered and are not. Taking
/// them together is what makes mismatching them an act rather than an
/// oversight.
#[derive(Debug)]
pub struct Sequencing {
    coordinator: Coordinator,
    index: IndexReader,
}

impl Sequencing {
    /// Pairs a coordinator with the reader `Coordinator::open` returned beside
    /// it.
    #[must_use]
    pub const fn new(coordinator: Coordinator, index: IndexReader) -> Self {
        Self { coordinator, index }
    }
}

/// Every seam a `Cluster` is built over besides `Sequencing`.
///
/// ⚠️ Bundled so [`Cluster::new`] stays under `rust-style.md`'s
/// argument-count limit as a third one joins `store`, not because these two
/// share `Sequencing`'s own "mismatching them is a bug" invariant.
#[derive(Debug)]
pub struct Seams {
    /// The object store this broker reads and writes through.
    pub store: Arc<dyn ObjectStore>,
    /// The consumer-group state machine's own seam (`M4.2`, `ADR-0034`).
    pub group_coordinator: Arc<dyn GroupCoordinator>,
    /// The committed-offset durability seam (`M4.14`, `ADR-0035`).
    pub group_metadata_log: Arc<dyn GroupMetadataLog>,
}

/// What a flush can fail with, in the two ways a client hears differently.
///
/// ⚠️ **The split is the point, not the taxonomy.** A write that never landed
/// and a write that landed but was never journalled are different events for a
/// client — the first is `NOT_ENOUGH_REPLICAS`, which every Kafka client
/// retries; the second is `LEADER_NOT_AVAILABLE`, which sends it to `Metadata`
/// and back — and different events for an operator, since only the second
/// leaves an object in the bucket that nothing references.
///
/// ⚠️ **The cause is carried, not flattened.** During an outage the wire code
/// says only "durability not achieved"; the reason the PUT failed has to be
/// renderable somewhere, which `error-handling.md` rule 3 is about.
#[derive(Debug, thiserror::Error)]
pub enum FlushError {
    /// The bundle could not be sealed, named, or written.
    #[error("the object could not be written: {0}")]
    Store(#[source] Error),
    /// It was written, but its position could not be committed.
    #[error("the object was written but its position was not committed: {0}")]
    Commit(#[source] CoordinatorError),
}

impl Cluster {
    /// A cluster advertising `host:port` as node 0.
    ///
    /// ⚠️ **`writer` is per process and there is no way to pass anything
    /// else** — see [`WriterId`]. `ADR-0026`'s unconditional `put` is safe
    /// only because no other live process writes the keys this one will.
    ///
    /// ⚠️ **`async` since `M4.14`, and no longer blocking on replay since
    /// `M4.15a`.** `M4.14` made this `async` to open
    /// `seams.group_metadata_log` and await its full replay before
    /// returning; `M4.15a` moves that replay into a background task
    /// (`tokio::spawn`) instead, because blocking meant there was no live
    /// window for `crate::fencing::NodeReadiness::load_in_progress` to
    /// ever be observed `true` — this `Cluster` now becomes queryable at
    /// once, answering `COORDINATOR_LOAD_IN_PROGRESS` to every fenced
    /// request until `replay_gate` reports ready. `Sequencing`'s own
    /// `Coordinator::open` remains the precedent for "replay before
    /// serving" on the *topic* log, done synchronously by the caller
    /// before `Cluster::new` — that log has no gate to answer through and
    /// is out of `M4.15a`'s own scope.
    ///
    /// ⚠️ **A replay failure is no longer returned from this function.**
    /// Before `M4.15a` it was: `CommittedOffsets::open`'s error propagated
    /// through `Cluster::new`'s own `Result`, failing broker startup
    /// outright. Now the replay runs after `new` has already returned
    /// `Ok`, so a failure instead leaves `replay_gate` permanently
    /// unready — every fenced request keeps answering
    /// `COORDINATOR_LOAD_IN_PROGRESS` forever rather than a wrong answer
    /// (`behavior.md` rule 11), but nothing surfaces *why* outside that
    /// wire code today. A real health/observability signal for this is
    /// not yet built (no `tracing` dependency exists in this crate,
    /// `rust-style.md` rule 12) — named here rather than silently
    /// assumed, not solved in this task.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyObjectKey`] or [`Error::MalformedWriterId`] if `writer`
    /// is not a usable key component, which [`WriterId::mint`] never
    /// produces.
    // ⚠️ `async` with no top-level `.await`, still — `M4.15c` added a
    // second replay (`crate::group_transitions`'s own) to the same
    // spawned task rather than awaiting it here directly, `tokio::join!`
    // running both replays concurrently before either marks
    // `replay_gate` ready. Kept `async` anyway rather than reverting
    // every call site's `.await` a second time.
    #[allow(clippy::unused_async)]
    pub async fn new(
        host: impl Into<String>,
        port: i32,
        sequencing: Sequencing,
        seams: Seams,
        writer: &WriterId,
    ) -> Result<Self, Error> {
        let committed_offsets = Arc::new(crate::offset_commit::CommittedOffsets::new_empty(
            Arc::clone(&seams.group_metadata_log),
        ));
        let replay_gate = Arc::new(crate::replay_gate::ReplayGate::new());
        let (group_transitions, group_transitions_task) =
            crate::group_transitions::GroupTransitions::new();
        let replay_task = tokio::spawn(replay::replay_groups(
            Arc::clone(&committed_offsets),
            Arc::clone(&replay_gate),
            group_transitions_task,
            Arc::clone(&seams.group_coordinator),
            Arc::clone(&seams.group_metadata_log),
        ));
        Ok(Self {
            node_id: 0,
            host: host.into(),
            port,
            // In-memory until a composer passes one (`with_catalog`).
            catalog: Arc::new(FakeTopicCatalog::new()),
            topics: Mutex::new(topics::TopicCache::default()),
            coordinator: sequencing.coordinator,
            index: sequencing.index,
            store: seams.store,
            region_sealer: Arc::new(RejectingRegionSealer),
            namer: Mutex::new(BundleNamer::new(writer.as_str())?),
            reaped_reads: AtomicU64::new(0),
            topic_lookups: AtomicU64::new(0),
            creator_topic_loads: Mutex::new(HashMap::new()),
            group_coordinator: seams.group_coordinator,
            group_joins: GroupJoins::default(),
            sync_groups: crate::sync_group::SyncGroups::default(),
            heartbeats: crate::heartbeat::Heartbeats::default(),
            committed_offsets,
            replay_gate,
            replay_task: Mutex::new(Some(replay_task)),
            group_transitions,
        })
    }

    /// This cluster, reading topics through `catalog` instead of the
    /// in-memory default. ⚠️ Call before serving: the cache starts empty
    /// either way, so nothing resolved against the default survives.
    #[must_use]
    pub fn with_catalog(mut self, catalog: Arc<dyn TopicCatalog>) -> Self {
        self.catalog = catalog;
        self.topics = Mutex::new(topics::TopicCache::default());
        self
    }

    /// Installs the encryption seam for customer-key topic writes.
    #[must_use]
    pub fn with_region_sealer(mut self, sealer: Arc<dyn RegionSealer>) -> Self {
        self.region_sealer = sealer;
        self
    }

    pub(crate) fn region_sealer(&self) -> &dyn RegionSealer {
        self.region_sealer.as_ref()
    }

    /// The consumer-group coordinator every connection's `JoinGroup`
    /// handler drives (`M4.7`).
    ///
    /// ⚠️ **Reads only, since `M4.15c`.** `.record(group)` is still safe
    /// to call directly (a plain read, synchronized by the coordinator's
    /// own internal lock); a *mutating* `.transition(...)` call must go
    /// through [`Cluster::group_transitions`] instead — `crate::fencing`
    /// and every handler's own test fixtures still read through here,
    /// `group_transitions.rs`'s own module doc names why the write path
    /// moved.
    pub(crate) fn group_coordinator(&self) -> &dyn GroupCoordinator {
        self.group_coordinator.as_ref()
    }

    /// The join-round bookkeeping every connection's `JoinGroup` handler
    /// shares (`M4.7`).
    pub(crate) const fn group_joins(&self) -> &GroupJoins {
        &self.group_joins
    }

    /// The sync-round bookkeeping every connection's `SyncGroup` handler
    /// shares (`M4.8`).
    pub(crate) const fn sync_groups(&self) -> &crate::sync_group::SyncGroups {
        &self.sync_groups
    }

    /// The session-timeout tracking every connection's `Heartbeat` handler
    /// shares, and `JoinGroup`'s own handler registers a fresh member
    /// into (`M4.9`).
    pub(crate) const fn heartbeats(&self) -> &crate::heartbeat::Heartbeats {
        &self.heartbeats
    }

    /// The committed-offset bookkeeping every connection's `OffsetCommit`
    /// handler shares (`M4.12`).
    pub(crate) fn committed_offsets(&self) -> &crate::offset_commit::CommittedOffsets {
        &self.committed_offsets
    }

    /// This coordinator's incarnation, which every watermark a client carries
    /// is fenced against (`ADR-0023`).
    #[must_use]
    pub const fn epoch(&self) -> CoordinatorEpoch {
        self.coordinator.epoch()
    }

    /// What this broker's index looks like to a freshness decision.
    ///
    /// ⚠️ **It is not a cache, and that is the whole reason this method
    /// exists rather than a bare `end_offset` call.** The index here is the
    /// coordinator's own — folded before the ack, by the only writer — so it
    /// is never behind what has been acknowledged and never silent, which is
    /// why `silent_for_ms` is zero. ⚠️ **A *follower*'s index is a cache** and
    /// will not be able to say that (`M7`); routing the decision through
    /// [`CacheState::admits`] now is what stops a follower quietly inheriting
    /// an answer only a coordinator may give — hazard H1, whose symptom is a
    /// `ListOffsets` below truth and a consumer lag that goes negative.
    #[must_use]
    pub fn cache_state(&self) -> CacheState {
        CacheState::new(self.epoch(), self.index.applied_upto(), 0)
    }

    /// How far this shard's index has folded, and a way to park until it
    /// folds further.
    ///
    /// ⚠️ **What a long-poll fetch waits on** (`M3.20`, `M3.md` task 17). One
    /// watch serves every partition, so a wakeup is "something committed", not
    /// "your partition committed" — the caller re-reads and parks again if it
    /// was not theirs.
    #[must_use]
    pub fn watch(&self) -> oqueue_coordinator::IndexWatch {
        self.coordinator.watch()
    }

    /// Where the next record for this partition lands — its high watermark,
    /// derived from the index and settable by nobody (`M3.md` task 13).
    #[must_use]
    pub fn high_watermark(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.index.end_offset(topic, partition)
    }

    /// The metadata log's handle — the write path's serialization point.
    pub(crate) const fn coordinator(&self) -> &Coordinator {
        &self.coordinator
    }

    /// The read-only index. ⚠️ **Read-only by type** (`ADR-0024`): the
    /// coordinator owns the writable side, and nothing here can fold.
    pub(crate) const fn index(&self) -> &IndexReader {
        &self.index
    }

    /// The object store this broker reads and writes through.
    pub(crate) fn store(&self) -> &Arc<dyn ObjectStore> {
        &self.store
    }

    /// How many reads have found an object the index named and the store did
    /// not have.
    ///
    /// ⚠️ **A number that should stay zero, and an alarm when it does not.**
    /// Object ids are never reused, so every one of these is a reaped object
    /// the index is still pointing at — doc 12 §4.6 says a nonzero rate means
    /// `M5`'s deletion delay is too short. It is a counter rather than a
    /// metric because `M9` owns the metrics surface; what matters now is that
    /// the number *exists* and something can read it.
    #[must_use]
    pub fn reaped_reads(&self) -> u64 {
        self.reaped_reads.load(Ordering::Relaxed)
    }

    pub(crate) fn count_reaped_read(&self) {
        self.reaped_reads.fetch_add(1, Ordering::Relaxed);
    }

    /// How many catalog entries [`Cluster::partition_count`] and
    /// [`Cluster::topic_names`] have together touched since this cluster
    /// was created — `M9.17`'s own cost-test proxy, read as a delta around
    /// one request rather than compared across clusters.
    #[must_use]
    pub fn topic_lookups(&self) -> u64 {
        self.topic_lookups.load(Ordering::Relaxed)
    }

    /// The namer minting this process's object keys.
    pub(crate) const fn namer(&self) -> &Mutex<BundleNamer> {
        &self.namer
    }
}

mod replay;
mod topics;

#[cfg(test)]
mod tests;
