//! What the integration tests need before they can ask a broker anything:
//! a real cluster, and a way to hold the coordinator loop alive.
//!
//! ⚠️ **A second fixture, and not a duplicate of the crate's own.**
//! `src/testing.rs` is `#[cfg(test)]`, which is invisible from an integration
//! test — a separate crate by construction. What is here is deliberately the
//! *composition-root* shape instead: only the public API, exactly what
//! `bin/oqueue` calls, so a change that broke the wiring would fail here
//! rather than only in a unit test that could reach past it.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is the only root of this
// test binary and nothing outside it can name these. The workspace's
// `unreachable_pub = "deny"` is right about library crates and has no way to
// tell a test binary's shared module apart from one.
#![allow(unreachable_pub)]

use oqueue_broker::{Cluster, Sequencing, WriterId};
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    CoordinatorEpoch, CountingObjectStore, FakeMaterializedIndex, FakeMetadataLog, FakeObjectStore,
    FaultMetadataLog, ObjectStore,
};
use std::sync::Arc;

/// The store an integration test can count operations against.
pub type TestStore = CountingObjectStore<FakeObjectStore>;

/// The log an integration test can make refuse.
///
/// ⚠️ **Wrapped for every broker rather than by a second constructor**, and
/// injecting nothing until told to. `M10.9` needs the *middle* crash point —
/// the object durable and the commit refused — and the alternative was a
/// parallel `broker_with_faulty_log` whose wiring could drift from this one's.
/// A fixture that differs from the fixture under test is how a fault test ends
/// up proving something about the fixture.
pub type TestLog = FaultMetadataLog<FakeMetadataLog>;

/// A cluster, its store, and the task serving its commits.
pub struct Broker {
    pub cluster: Arc<Cluster>,
    pub store: Arc<TestStore>,
    /// The metadata log behind the coordinator, so a test can refuse a commit.
    pub log: Arc<TestLog>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

/// A broker hosting each of `topics`.
pub async fn broker(topics: &[&str]) -> Broker {
    let store = Arc::new(CountingObjectStore::new(FakeObjectStore::new()));
    let log = Arc::new(FaultMetadataLog::new(FakeMetadataLog::new()));
    let index = Box::new(FakeMaterializedIndex::new());
    let (coordinator, serving, reader) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn oqueue_core::MetadataLog>,
        index,
        CoordinatorEpoch::new(1),
    )
    .await
    .expect("an empty log opens");
    let shared: Arc<dyn ObjectStore> = Arc::clone(&store) as Arc<dyn ObjectStore>;
    let sequencing = Sequencing::new(coordinator, reader);
    let cluster = Cluster::new(
        "h",
        1,
        sequencing,
        oqueue_broker::Seams {
            store: shared,
            group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
        },
        &WriterId::mint(),
    )
    .expect("a minted identity is a usable key component");
    for topic in topics {
        cluster.ensure_topic(topic);
    }
    Broker {
        cluster: Arc::new(cluster),
        store,
        log,
        serving: tokio::spawn(serving.run()),
    }
}

/// The dependency's own `RecordBatchEncoder` producing a batch of `values` —
/// the same authority `oqueue-codec`'s golden tests use (`ADR-0017`).
pub fn golden_batch(values: &[&'static [u8]]) -> Vec<u8> {
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
