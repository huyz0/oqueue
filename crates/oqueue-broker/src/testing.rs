//! Fixtures every handler's tests share: a real cluster over fakes, and a
//! record batch the dependency itself encoded.
//!
//! ⚠️ **`cfg(test)` and `pub(crate)`, because a `#[cfg(test)]` helper is
//! invisible across crates** and produce, fetch and the ingest rule all need
//! the same two things. `ADR-0017`'s rule stands: a golden fixture comes from
//! the dependency's own encoder, never from bytes this repository wrote down.
//!
//! ⚠️ **The cluster here is real in every part that M3 built.** A real
//! [`Coordinator`], a real fold, a real bundle, a real object store call —
//! only the *engines* are fakes, and both are fakes the composition root uses
//! too, because M3 ships no durable metadata log (`M3.md`'s goal note, `M6`).
//! A test that passed against a stub cluster and fails here is telling the
//! truth.

#![allow(clippy::expect_used)]
// ⚠️ Clippy calls the inner `pub(crate)` redundant while `unreachable_pub`
// refuses the alternative — the same standoff `produce.rs` and codec's
// `batch.rs` already resolved this way. A `#[cfg(test)]` module cannot be
// `pub`, so `pub(crate)` is the only thing that compiles.
#![allow(clippy::redundant_pub_crate)]

use crate::cluster::Cluster;
use crate::writer_id::WriterId;
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    CoordinatorEpoch, CountingObjectStore, FakeGroupMetadataLog, FakeMaterializedIndex,
    FakeMetadataLog, FakeObjectStore, FaultConfig, FaultGroupMetadataLog, ObjectStore, PartitionId,
    StormKind, TopicId,
};

/// The store every fixture holds: a real fake, counted.
///
/// ⚠️ **Counted by default rather than on request**, because the claims this
/// milestone has to make are about *how many* store calls a handler issues —
/// FR-32's one PUT, FR-12's zero GETs — and a fixture that had to be opted
/// into would leave them argued instead of asserted.
pub(crate) type TestStore = CountingObjectStore<FakeObjectStore>;
/// The group metadata log every fixture holds: a real fake, fault-capable —
/// `TestStore`'s own "capable by default, not on request" precedent, so a
/// test asserting `M4.14`'s own "impossible by construction" claim does not
/// need a second fixture shape.
pub(crate) type TestGroupMetadataLog = FaultGroupMetadataLog<FakeGroupMetadataLog>;
use std::sync::Arc;

/// A cluster and the coordinator loop serving it.
///
/// ⚠️ **The loop's handle is held, not detached** — `async-concurrency.md`
/// rule 13. Dropping it aborts the loop, so every `Cluster` here outlives the
/// task that answers its commits exactly as long as the test holds this.
pub(crate) struct Fixture {
    pub(crate) cluster: Arc<Cluster>,
    pub(crate) store: Arc<TestStore>,
    /// One connection's memory, so a test can produce and then read on it.
    pub(crate) session: crate::session::Session,
    /// The concrete fake behind `cluster`'s own type-erased
    /// `Arc<dyn GroupCoordinator>` — `M4.10`'s own need: a test asserting
    /// "exactly one rebalance" needs `transition_calls()`, which no method
    /// on the trait itself exposes.
    pub(crate) group_coordinator: Arc<oqueue_core::FakeGroupCoordinator>,
    /// The concrete, fault-capable fake behind `cluster`'s own type-erased
    /// `Arc<dyn GroupMetadataLog>` (`M4.14`) — a fault-injection test tells
    /// it to refuse the next append; a restart test opens a *second*
    /// `CommittedOffsets::open` from this same log to prove replay.
    pub(crate) group_metadata_log: Arc<TestGroupMetadataLog>,
    serving: tokio::task::JoinHandle<()>,
}

impl Fixture {
    /// Makes the next `put` write the object durably and *then* fail.
    ///
    /// ⚠️ **Not the same fault as [`break_store`](Self::break_store), and the
    /// difference is FR-10's whole subject.** A storm fails the call *before*
    /// the write, so nothing lands; this lands the bytes and loses the
    /// acknowledgement — `ADR-0005` guarantee 2's unknown state, and the
    /// "kill between PUT and ack" that `requirements.md` names as FR-10's
    /// verification method. A broker that treated the two alike would be right
    /// about one of them by luck.
    pub(crate) fn lose_the_next_ack(&self) {
        self.store.inner().set_faults(FaultConfig {
            crash_after_put_before_ack: 1,
            ..FaultConfig::default()
        });
    }

    /// Makes the next `calls` store operations fail with a 503 storm.
    ///
    /// ⚠️ **After the fixture exists**, which `with_broken_store` cannot do:
    /// a read path needs something written before the store starts failing,
    /// and a store broken from the start has nothing to read.
    ///
    /// ⚠️ **Nothing lands.** The call fails before the write — which is what
    /// [`lose_the_next_ack`](Self::lose_the_next_ack) is the other half of.
    /// Makes every store call pend `polls` times before resolving.
    ///
    /// ⚠️ **A read that takes real time, deterministically.** `read_all` awaits
    /// object-storage GETs, so a commit can land *during* it — and whether the
    /// handler notices depends on sampling its baseline before the read rather
    /// than after. Nothing else in the suite can hold a read open long enough
    /// for that window to exist. The fake self-wakes on each `Pending`, so this
    /// costs polls rather than wall-clock.
    pub(crate) fn slow_store(&self, polls: u32) {
        self.store.inner().set_faults(FaultConfig {
            latency_polls: polls,
            ..FaultConfig::default()
        });
    }

    /// Puts the store back to answering immediately.
    pub(crate) fn heal_store(&self) {
        self.store.inner().set_faults(FaultConfig::default());
    }

    pub(crate) fn break_store(&self, calls: u32) {
        self.store.inner().set_faults(FaultConfig {
            storm: Some((StormKind::Transient, calls)),
            ..FaultConfig::default()
        });
    }

    /// Makes every following `GroupMetadataLog::append` refuse, until
    /// [`Self::heal_group_metadata_log`] is called — `M4.14`'s own
    /// fault-injection test: a refused append must leave nothing durable
    /// and nothing acknowledged.
    pub(crate) fn refuse_group_metadata_append(&self) {
        self.group_metadata_log.refuse_append();
    }

    /// Puts the group metadata log back to answering immediately.
    pub(crate) fn heal_group_metadata_log(&self) {
        self.group_metadata_log.heal();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// A cluster hosting each of `topics`, over an empty log and store.
pub(crate) async fn fixture(topics: &[&str]) -> Fixture {
    at("h", 1, topics).await
}

/// A cluster whose store fails the next `calls` operations with a 503 storm.
///
/// ⚠️ **The failure paths need a fixture as much as the happy one does.** A
/// produce whose PUT never landed must not be acknowledged, and a fetch whose
/// GET failed must not answer an empty partition — neither is assertable
/// against a store that always works, and both are the outcomes doc 12 §4.6
/// calls silent wrongness.
pub(crate) async fn with_broken_store(topics: &[&str], calls: u32) -> Fixture {
    let store = Arc::new(CountingObjectStore::new(FakeObjectStore::with_faults(
        FaultConfig {
            storm: Some((StormKind::Transient, calls)),
            ..FaultConfig::default()
        },
    )));
    with_store("h", 1, topics, store).await
}

/// A cluster whose coordinator loop has stopped, so nothing can be committed.
///
/// ⚠️ **A produce here must be refused, not acknowledged.** The object may
/// well have landed; what did not happen is the commit, and an offset handed
/// out without one is an offset a restart unmakes.
pub(crate) async fn with_dead_coordinator(topics: &[&str]) -> Fixture {
    let fixture = fixture(topics).await;
    fixture.serving.abort();
    // ⚠️ Yield until the abort has actually taken effect: `abort()` only marks
    // the task, and a commit racing it would be served rather than refused.
    while !fixture.serving.is_finished() {
        tokio::task::yield_now().await;
    }
    fixture
}

/// The same, advertising a chosen identity — what `Metadata` hands out.
pub(crate) async fn at(host: &str, port: i32, topics: &[&str]) -> Fixture {
    with_store(
        host,
        port,
        topics,
        Arc::new(CountingObjectStore::new(FakeObjectStore::new())),
    )
    .await
}

/// The same, over a store the caller can reach — to count operations, or to
/// break one.
pub(crate) async fn with_store(
    host: &str,
    port: i32,
    topics: &[&str],
    store: Arc<TestStore>,
) -> Fixture {
    let log = Arc::new(FakeMetadataLog::new());
    let index = Box::new(FakeMaterializedIndex::new());
    let epoch = CoordinatorEpoch::new(1);
    let (coordinator, serving, reader) = Coordinator::open(log, index, epoch)
        .await
        .expect("an empty log opens");
    let shared: Arc<dyn ObjectStore> = Arc::clone(&store) as Arc<dyn ObjectStore>;
    let sequencing = crate::cluster::Sequencing::new(coordinator, reader);
    let group_coordinator = Arc::new(oqueue_core::FakeGroupCoordinator::new());
    let group_metadata_log = Arc::new(TestGroupMetadataLog::new(FakeGroupMetadataLog::new()));
    let cluster = Cluster::new(
        host,
        port,
        sequencing,
        crate::cluster::Seams {
            store: shared,
            group_coordinator: Arc::clone(&group_coordinator)
                as Arc<dyn oqueue_core::GroupCoordinator>,
            group_metadata_log: Arc::clone(&group_metadata_log)
                as Arc<dyn oqueue_core::GroupMetadataLog>,
        },
        &WriterId::mint(),
    )
    .await
    .expect("a minted identity is a usable key component, and an empty log opens");
    for topic in topics {
        cluster.ensure_topic(topic);
    }
    Fixture {
        cluster: Arc::new(cluster),
        session: crate::session::Session::default(),
        store,
        group_coordinator,
        group_metadata_log,
        serving: tokio::spawn(serving.run()),
    }
}

/// Produces one batch into `topic`'s partition 0 through the real handler,
/// panicking unless every partition was accepted.
///
/// ⚠️ **The produce path is the fixture for the read path**, deliberately: a
/// fetch test that seeded the index by hand would pass against a broker whose
/// produce wrote something else.
pub(crate) async fn produce_one(fixture: &Fixture, name: &str, records: Vec<u8>) {
    use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
    use kafka_protocol::messages::{ProduceRequest, TopicName};
    use kafka_protocol::protocol::{Encodable, StrBytes};

    let mut request = ProduceRequest::default();
    request.acks = -1;
    let mut t = TopicProduceData::default();
    t.name = TopicName(StrBytes::from_string(name.to_owned()));
    let mut p = PartitionProduceData::default();
    p.index = 0;
    p.records = Some(bytes::Bytes::from(records));
    t.partition_data.push(p);
    request.topic_data.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, 9).expect("encodes");

    let prelude = oqueue_codec::frame::RequestPrelude {
        api_key: 0,
        api_version: 9,
        correlation_id: 1,
    };
    let crate::connection::HandlerResponse::Reply(_) = crate::produce::handle(
        &fixture.cluster,
        &fixture.session,
        prelude,
        &body,
        &crate::authz::AuthzContext {
            principal: None,
            credentials_configured: false,
            topic_grants: &oqueue_core::TopicGrants::default(),
        },
    )
    .await
    else {
        panic!("the fixture produce replies");
    };
}

/// A topic id from a name a test wrote.
pub(crate) fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

/// A partition id from an index a test wrote.
pub(crate) fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

/// The dependency's own `RecordBatchEncoder` producing a two-record batch —
/// the same authority `oqueue-codec`'s golden tests use (`ADR-0017`).
pub(crate) fn golden_batch() -> Vec<u8> {
    golden_batch_of(&[b"hello", b"world"])
}

/// The same, with the values chosen — so two batches in one bundle are
/// distinguishable when a test reads them back.
pub(crate) fn golden_batch_of(values: &[&'static [u8]]) -> Vec<u8> {
    use kafka_protocol::records::{
        Compression, Record, RecordBatchEncoder, RecordEncodeOptions, TimestampType,
    };
    // offset - sequence must match across records or the encoder splits them
    // into separate batches (its grouping predicate).
    fn record(offset: i64, value: &'static [u8]) -> Record {
        Record {
            transactional: false,
            control: false,
            partition_leader_epoch: 0,
            producer_id: -1,
            producer_epoch: -1,
            timestamp_type: TimestampType::Creation,
            offset,
            sequence: i32::try_from(offset).unwrap_or(0),
            delete_horizon: false,
            timestamp: 1_700_000_000_000 + offset,
            key: None,
            value: Some(bytes::Bytes::copy_from_slice(value)),
            headers: kafka_protocol::indexmap::IndexMap::default(),
        }
    }
    let records: Vec<Record> = values
        .iter()
        .enumerate()
        .map(|(n, value)| record(i64::try_from(n).unwrap_or(0), value))
        .collect();
    let mut buf = bytes::BytesMut::new();
    RecordBatchEncoder::encode(
        &mut buf,
        &records,
        &RecordEncodeOptions {
            version: 2,
            compression: Compression::None,
        },
    )
    .expect("the dependency encodes its own records");
    buf.to_vec()
}
