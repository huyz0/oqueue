//! The in-memory single-node cluster the M2 APIs serve against.
//!
//! ⚠️ **A stub, and named one everywhere** — M2's completion condition is a
//! real client producing and fetching against *a stub partition*
//! (`M2.md`); durable storage, offsets and the index are `M3`'s. Batches
//! land here as the opaque bytes the wire carried, exactly the shape the
//! store will take.
//!
//! ## Broker identity, decided rather than convenient
//!
//! One node, id 0, advertising the host and port the composition root
//! passed in — never something inferred. Doc 02 §7.2 records the trap this
//! sidesteps for now: clients treat node id *and* hostname as identity, so
//! a fleet of interchangeable processes needs a deliberate identity scheme
//! (`WarpStream`'s DNS-case trick). A single-node M2 has a single honest
//! identity; the fleet answer belongs to the milestone that has a fleet.

use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

/// One partition: batches as received, and the offset the next one gets.
#[derive(Debug, Default)]
pub struct StubPartition {
    /// Record batches, exactly as the wire carried them.
    pub batches: Vec<Vec<u8>>,
    /// The offset the next produced batch's first record is assigned.
    pub next_offset: i64,
}

/// One topic: its id and its partitions. Ids exist because the modern
/// wire addresses topics by uuid — `Produce` v13, `Fetch` v13+, and
/// `Metadata` from v10 all carry them.
#[derive(Debug)]
pub struct StubTopic {
    /// The topic's id: deterministic (creation order), never nil.
    pub id: Uuid,
    /// The topic's partitions.
    pub partitions: Vec<StubPartition>,
}

/// The whole stub cluster: identity plus topics.
#[derive(Debug)]
pub struct StubCluster {
    /// This broker's node id — 0, the only node.
    pub node_id: i32,
    /// The advertised host, verbatim from the composition root.
    pub host: String,
    /// The advertised port.
    pub port: i32,
    // ⚠️ A std `Mutex`, deliberately (`async-concurrency.md` rules 6 and 8):
    // every critical section is a map touch with no `.await` inside, and
    // the lock protects data, never control flow.
    topics: Mutex<HashMap<String, StubTopic>>,
}

impl StubCluster {
    /// A cluster advertising `host:port` as node 0.
    #[must_use]
    pub fn new(host: impl Into<String>, port: i32) -> Self {
        Self {
            node_id: 0,
            host: host.into(),
            port,
            topics: Mutex::new(HashMap::new()),
        }
    }

    /// The topic's partition count, or `None` if it does not exist.
    #[must_use]
    pub fn partition_count(&self, topic: &str) -> Option<usize> {
        let topics = self
            .topics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        topics.get(topic).map(|t| t.partitions.len())
    }

    /// The topic's id, or `None` if it does not exist.
    #[must_use]
    pub fn topic_id(&self, topic: &str) -> Option<Uuid> {
        let topics = self
            .topics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        topics.get(topic).map(|t| t.id)
    }

    /// The name behind a topic id, or `None` — how the id-addressed APIs
    /// (`Produce` v13, `Fetch` v13+) resolve their targets.
    #[must_use]
    pub fn topic_name_by_id(&self, id: Uuid) -> Option<String> {
        let topics = self
            .topics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        topics
            .iter()
            .find(|(_, t)| t.id == id)
            .map(|(name, _)| name.clone())
    }

    /// Creates `topic` with one partition if absent. Returns whether it
    /// exists afterwards (always true; the return shape leaves room for a
    /// creation policy later).
    pub fn ensure_topic(&self, topic: &str) -> bool {
        let mut topics = self
            .topics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Creation-order ids, starting at 1: deterministic, never nil
        // (nil is the wire's "no id" sentinel), unique because topics are
        // never removed.
        let next_id = Uuid::from_u128(topics.len() as u128 + 1);
        topics.entry(topic.to_owned()).or_insert_with(|| StubTopic {
            id: next_id,
            partitions: vec![StubPartition::default()],
        });
        true
    }

    /// Every topic name, sorted — `Metadata` with no filter asks for all.
    #[must_use]
    pub fn topic_names(&self) -> Vec<String> {
        let mut names: Vec<String> = {
            let topics = self
                .topics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            topics.keys().cloned().collect()
        };
        names.sort();
        names
    }

    /// Appends a batch to `topic`/`partition`, returning the base offset
    /// assigned — or `None` if the topic or partition does not exist.
    pub fn append(
        &self,
        topic: &str,
        partition: usize,
        batch: Vec<u8>,
        records: i64,
    ) -> Option<i64> {
        self.append_with(topic, partition, records, |_| batch)
    }

    /// [`Self::append`], with `make` handed the assigned base offset and
    /// returning the batch to store — *inside the critical section*, because
    /// produce's offset rewrite must be atomic with the reservation, or two
    /// in-flight produces could each stamp the other's offset. The closure
    /// runs under the lock; it must not block (`async-concurrency.md`
    /// rule 6).
    pub fn append_with(
        &self,
        topic: &str,
        partition: usize,
        records: i64,
        make: impl FnOnce(i64) -> Vec<u8>,
    ) -> Option<i64> {
        let mut topics = self
            .topics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let p = topics.get_mut(topic)?.partitions.get_mut(partition)?;
        let base = p.next_offset;
        p.next_offset += records.max(1);
        p.batches.push(make(base));
        drop(topics);
        Some(base)
    }

    /// ⚠️ **Every batch, from the beginning, regardless of
    /// `fetch_offset`** — this stub stores opaque batch bytes and holds no
    /// per-batch base offsets to filter on, so a fetch may redeliver from
    /// zero; `M2.24`'s handler owns saying so honestly, and real offset
    /// resolution is `M3`'s index. The high watermark rides along.
    #[must_use]
    pub fn read(
        &self,
        topic: &str,
        partition: usize,
        _fetch_offset: i64,
    ) -> Option<(Vec<Vec<u8>>, i64)> {
        let topics = self
            .topics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let p = topics.get(topic)?.partitions.get(partition)?;
        let result = (p.batches.clone(), p.next_offset);
        drop(topics);
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    // Test-only: every expect is on state this test just created.
    #![allow(clippy::expect_used)]

    use super::StubCluster;

    #[test]
    fn topics_appear_once_and_offsets_advance() {
        let cluster = StubCluster::new("localhost", 9092);
        assert_eq!(cluster.partition_count("t"), None);
        assert!(cluster.ensure_topic("t"));
        assert!(cluster.ensure_topic("t"), "idempotent");
        assert_eq!(cluster.partition_count("t"), Some(1));
        assert_eq!(cluster.append("t", 0, vec![1], 2), Some(0));
        assert_eq!(cluster.append("t", 0, vec![2], 3), Some(2));
        let (batches, high) = cluster.read("t", 0, 0).expect("exists");
        assert_eq!(batches.len(), 2);
        assert_eq!(high, 5);
        assert_eq!(cluster.append("missing", 0, vec![], 1), None);
    }

    #[test]
    fn topic_ids_are_stable_unique_and_resolvable() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("a");
        cluster.ensure_topic("b");
        cluster.ensure_topic("a"); // idempotent: no id churn
        let a = cluster.topic_id("a").expect("a has an id");
        let b = cluster.topic_id("b").expect("b has an id");
        assert_ne!(a, b);
        assert_ne!(a, super::Uuid::nil());
        assert_eq!(cluster.topic_name_by_id(a).as_deref(), Some("a"));
        assert_eq!(cluster.topic_name_by_id(super::Uuid::nil()), None);
        assert_eq!(cluster.topic_id("ghost"), None);
    }
}
