//! NFR-4 (`M7.7`): produce cost does not grow with the topic count.
//!
//! ⚠️ **Work counts, not latency** (`ADR-0049` point 5, `testing.md` rule 11):
//! catalog calls of every kind, `Cluster::topic_lookups`, allocations, and
//! object-store operations for one produce request, each identical over a
//! catalog of 1,000,000 and of 10,000,000 topics. NFR-4's own verification is
//! a benchmark, which M14 re-measures in time; what is asserted here is that
//! nothing on the produce path does work that grows with the topic count.
//!
//! ⚠️ **Compared with no tolerance**, as `metadata.rs` is: fresh node per
//! size after a warm-up, fixed-width names, and identical counts in every run.

use crate::catalog::{Calls, Counted, SyntheticCatalog};
use crate::memory::{node_over, produce};
use oqueue_broker::{Cluster, Dispatcher};
use oqueue_core::{CountingObjectStore, FakeObjectStore, ObjectStore, Operation};
use std::sync::Arc;

/// Topics produced to: the same indices at both sizes.
const ACTIVE: u64 = 50;

/// What one produce request did.
#[derive(Debug, PartialEq, Eq)]
struct Cost {
    topic_lookups: u64,
    catalog: Calls,
    allocations: usize,
    /// Gets, puts and deletes: what `CountingObjectStore` counts.
    store: [u64; 3],
}

fn store_ops(store: &CountingObjectStore<FakeObjectStore>) -> [u64; 3] {
    let counts = store.counts();
    [Operation::Get, Operation::Put, Operation::Delete].map(|op| counts.count(op))
}

/// One produce to each [`ACTIVE`] topic, measured around the requests alone.
async fn produce_all(
    dispatcher: &Dispatcher,
    cluster: &Cluster,
    catalog: &Counted<SyntheticCatalog>,
    store: &CountingObjectStore<FakeObjectStore>,
) -> Cost {
    let (lookups, calls, ops) = (cluster.topic_lookups(), catalog.calls(), store_ops(store));
    let allocations = crate::allocations();
    for index in 0..ACTIVE {
        produce(dispatcher, cluster, &SyntheticCatalog::name(index)).await;
    }
    let allocations = crate::allocations() - allocations;
    let after = store_ops(store);
    Cost {
        topic_lookups: cluster.topic_lookups() - lookups,
        catalog: catalog.calls().since(calls),
        allocations,
        store: [0, 1, 2].map(|i| after[i] - ops[i]),
    }
}

/// The first produce to each active topic (the node resolves them), then a
/// second (steady state: all already served), on a fresh node over a
/// `size`-topic catalog.
async fn costs(size: u64) -> (Cost, Cost) {
    let catalog = Arc::new(Counted::new(SyntheticCatalog::new(size)));
    let store = Arc::new(CountingObjectStore::new(FakeObjectStore::new()));
    let (cluster, serving) = node_over(
        Arc::clone(&catalog) as _,
        Arc::clone(&store) as Arc<dyn ObjectStore>,
    )
    .await;
    let dispatcher = Dispatcher::new(Arc::clone(&cluster));
    let first = produce_all(&dispatcher, &cluster, &catalog, &store).await;
    let steady = produce_all(&dispatcher, &cluster, &catalog, &store).await;
    serving.abort();
    (first, steady)
}

#[tokio::test]
async fn produce_cost_is_flat_across_a_tenfold_topic_count() {
    let _serial = crate::serial().await;
    // Warm-up: one-time growth lands here, not in the first size measured.
    let _ = costs(1_000).await;

    let (small_first, small_steady) = costs(1_000_000).await;
    let (large_first, large_steady) = costs(10_000_000).await;
    println!("first produce: 1M {small_first:?}; 10M {large_first:?}");
    println!("steady produce: 1M {small_steady:?}; 10M {large_steady:?}");
    assert_eq!(
        small_first, large_first,
        "a first produce's cost must not grow with the topic count"
    );
    assert_eq!(
        small_steady, large_steady,
        "a steady-state produce's cost must not grow with the topic count"
    );
    assert_eq!(
        large_steady.catalog,
        Calls::default(),
        "a produce to served topics is answered from the node's cache: no catalog call"
    );
    assert!(
        large_first.store[1] > 0,
        "the produces reached the object store, so the store counts constrain something"
    );
}
