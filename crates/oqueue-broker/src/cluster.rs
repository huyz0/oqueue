//! The cluster the API handlers serve against: a topic registry, a
//! coordinator, an index and an object store.
//!
//! ⚠️ **This replaces `M2`'s `StubCluster`, and only half of what that held
//! was a stub.** The topic *registry* — names, ids, partition counts,
//! create-on-produce — is unchanged and moved here verbatim, because topic
//! administration is nobody's milestone yet: `M3` sequences offsets and
//! indexes objects, and a `CreateTopics` API with a creation policy is later.
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
//! composer. Every partition in one `Produce` request goes into one
//! [`BundleBuilder`](oqueue_core::BundleBuilder), so a request spanning N
//! topics costs **one** PUT and one
//! metadata record rather than N of each. Doc 12 prices a PUT far above the
//! bytes in it, so this ratio is the cost model.

use crate::join_group::GroupJoins;
use crate::writer_id::WriterId;
use oqueue_coordinator::{Coordinator, CoordinatorError};
use oqueue_core::{
    BundleNamer, CacheState, CoordinatorEpoch, Error, GroupCoordinator, IndexReader, ObjectStore,
    Offset, PartitionId, TopicId,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use uuid::Uuid;

/// One topic: its id and how many partitions it has.
#[derive(Debug)]
struct TopicEntry {
    id: Uuid,
    partitions: usize,
}

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
    // ⚠️ A std `Mutex`, deliberately (`async-concurrency.md` rules 6 and 8):
    // every critical section below is a map touch or a counter bump with no
    // `.await` inside, and the lock protects data, never control flow.
    topics: Mutex<HashMap<String, TopicEntry>>,
    coordinator: Coordinator,
    index: IndexReader,
    store: Arc<dyn ObjectStore>,
    namer: Mutex<BundleNamer>,
    reaped_reads: AtomicU64,
    /// How many catalog entries [`Cluster::partition_count`] and
    /// [`Cluster::topic_names`] have together touched — `M9.17`'s own
    /// cost-test proxy for NFR-12 (`docs/internal/standards/testing.md` rule
    /// 11 forbids asserting on wall-clock duration, so this is what a test
    /// asserting "CPU independent of catalog size" actually counts): one per
    /// `partition_count` call, and the full catalog size per `topic_names`
    /// call — the two shapes of cost a `Metadata` handler can have, targeted
    /// (O(topics a caller resolves)) and a full scan (O(topics that exist)).
    topic_lookups: AtomicU64,
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
    /// # Errors
    ///
    /// [`Error::EmptyObjectKey`] or [`Error::MalformedWriterId`] if `writer`
    /// is not a usable key component, which [`WriterId::mint`] never produces.
    pub fn new(
        host: impl Into<String>,
        port: i32,
        sequencing: Sequencing,
        seams: Seams,
        writer: &WriterId,
    ) -> Result<Self, Error> {
        Ok(Self {
            node_id: 0,
            host: host.into(),
            port,
            topics: Mutex::new(HashMap::new()),
            coordinator: sequencing.coordinator,
            index: sequencing.index,
            store: seams.store,
            namer: Mutex::new(BundleNamer::new(writer.as_str())?),
            reaped_reads: AtomicU64::new(0),
            topic_lookups: AtomicU64::new(0),
            group_coordinator: seams.group_coordinator,
            group_joins: GroupJoins::default(),
        })
    }

    /// The consumer-group coordinator every connection's `JoinGroup`
    /// handler drives (`M4.7`).
    pub(crate) fn group_coordinator(&self) -> &dyn GroupCoordinator {
        self.group_coordinator.as_ref()
    }

    /// The join-round bookkeeping every connection's `JoinGroup` handler
    /// shares (`M4.7`).
    pub(crate) const fn group_joins(&self) -> &GroupJoins {
        &self.group_joins
    }

    /// The topic's partition count, or `None` if it does not exist.
    #[must_use]
    pub fn partition_count(&self, topic: &str) -> Option<usize> {
        self.topic_lookups.fetch_add(1, Ordering::Relaxed);
        self.with_topics(|topics| topics.get(topic).map(|t| t.partitions))
    }

    /// The topic's id, or `None` if it does not exist.
    #[must_use]
    pub fn topic_id(&self, topic: &str) -> Option<Uuid> {
        self.with_topics(|topics| topics.get(topic).map(|t| t.id))
    }

    /// The name behind a topic id, or `None` — how the id-addressed APIs
    /// (`Produce` v13, `Fetch` v13+) resolve their targets.
    #[must_use]
    pub fn topic_name_by_id(&self, id: Uuid) -> Option<String> {
        self.with_topics(|topics| {
            topics
                .iter()
                .find(|(_, t)| t.id == id)
                .map(|(name, _)| name.clone())
        })
    }

    /// Every topic name, sorted — `Metadata` with no filter asks for all.
    ///
    /// ⚠️ **O(catalog), not O(what a caller keeps)** — this touches every
    /// entry to build and sort the list, unlike [`Cluster::partition_count`]'s
    /// one targeted lookup. `topic_lookups` below counts this proportionally
    /// to catalog size for exactly that reason: a caller that resolves a
    /// scoped set of names via `partition_count` per name costs O(that set);
    /// a caller that calls this and filters afterward is the O(catalog)
    /// anti-pattern `M9.10`'s own `all_topics_names` exists to avoid, and
    /// `M9.17`'s cost test needs a proxy that actually tells the two apart.
    #[must_use]
    pub fn topic_names(&self) -> Vec<String> {
        let mut names = self.with_topics(|topics| topics.keys().cloned().collect::<Vec<_>>());
        self.topic_lookups.fetch_add(
            u64::try_from(names.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        names.sort();
        names
    }

    /// Creates `topic` with one partition if absent. Returns whether it exists
    /// afterwards (always true; the return shape leaves room for a creation
    /// policy later).
    pub fn ensure_topic(&self, topic: &str) -> bool {
        let mut topics = self.topics.lock().unwrap_or_else(PoisonError::into_inner);
        // Creation-order ids, starting at 1: deterministic, never nil (nil is
        // the wire's "no id" sentinel), unique because topics are never
        // removed.
        let next_id = Uuid::from_u128(topics.len() as u128 + 1);
        topics.entry(topic.to_owned()).or_insert(TopicEntry {
            id: next_id,
            partitions: 1,
        });
        true
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

    fn with_topics<T>(&self, read: impl FnOnce(&HashMap<String, TopicEntry>) -> T) -> T {
        let topics = self.topics.lock().unwrap_or_else(PoisonError::into_inner);
        read(&topics)
    }
}
