//! Composing the broker from its seams: the coordinator, the index, the
//! store, and the retention task that runs beside them.
//!
//! ⚠️ **Split from `serve.rs` at the 500-line limit, along the concept**
//! (`M5.91`): that file binds and serves; this one decides what is served.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::ObjectStore;
use std::sync::Arc;

/// Composes the broker: a coordinator over a metadata log, a materialized
/// index, and the chosen object store.
///
/// ⚠️ **The one place the concrete materialization is chosen** (FR-50).
/// `oqueue-index`'s `MemoryIndex` is `M3`'s answer and not the project's — doc
/// 10 #12's disk engine is open — and it is picked here rather than defaulted
/// inside a library, so swapping it is one line in a composition root.
///
/// ⚠️ **Both logs live in the chosen object store** (`ADR-0046`, `M6.6`): the
/// metadata log under `meta/0`, the group log under `groups/0`, so positions
/// and committed offsets survive the process exactly as far as the store
/// does. Over the in-memory store that is not at all, which `serve`'s own
/// durability warning already tells an operator.
///
/// ⚠️ **The coordinator's loop is returned, not spawned here.** It is spawned
/// by `serve`, which watches its handle — `async-concurrency.md` rule 13 wants
/// an owner that can *observe* the task, and a `JoinHandle` dropped on the
/// floor observes nothing. A loop that panicked would otherwise leave a process
/// that stays up, accepts connections, and answers every produce
/// `LEADER_NOT_AVAILABLE` forever while no supervisor restarts it, because it
/// never exits and says nothing.
/// What composing a broker produces: the cluster, and the tasks `serve`
/// spawns and owns beside it.
pub(crate) struct Built {
    pub(crate) cluster: oqueue_broker::Cluster,
    /// The coordinator's loop, which replays its log before serving.
    pub(crate) serving: oqueue_coordinator::CoordinatorLoop,
    pub(crate) retention: oqueue_broker::Retention,
    /// The coordinator's current view of the metadata log, for the
    /// checkpoint cadence (`M6.17`).
    pub(crate) log: oqueue_broker::CurrentLog,
    /// The shard's leadership lease the loop is fenced by (`M6.7`).
    pub(crate) lease: Arc<oqueue_core::ObjectStoreLease>,
}

pub(crate) async fn build_cluster(
    host: String,
    port: i32,
    store: Arc<dyn ObjectStore>,
) -> std::io::Result<Built> {
    // ⚠️ **Nothing here waits on the store** (`M6.10`): both logs open on
    // first use and the coordinator replays inside its loop, so a node whose
    // store is unreachable still boots, answers what it can, and recovers
    // when the store does, with no restart.
    let (durable, group_metadata_log) = deferred_logs(&store);
    let log: Arc<dyn oqueue_core::MetadataLog> = Arc::clone(&durable) as _;
    let clock: Arc<dyn oqueue_core::Clock> = Arc::new(crate::wall_clock::WallClock);
    let writer = oqueue_broker::WriterId::mint();
    let lease = lease_for(&store, &writer, &clock).await;
    let (coordinator, serving, reader) = oqueue_coordinator::Coordinator::open_deferred(
        Arc::clone(&log),
        Box::new(oqueue_index::MemoryIndex::new()),
        oqueue_core::CoordinatorEpoch::new(1),
        Arc::clone(&clock),
    );
    let current: oqueue_broker::CurrentLog = Arc::new(std::sync::Mutex::new(Arc::clone(&durable)));
    let serving = serving
        .fenced_by(Arc::clone(&lease))
        .reopening_with(reopener(&store, &current));
    // FR-33: retention runs on time alone, over its own follower index.
    let retention = oqueue_broker::Retention::new(
        coordinator.clone(),
        log,
        Box::new(oqueue_index::MemoryIndex::new()),
        Arc::clone(&store),
        clock,
    )
    .map_err(|error| std::io::Error::other(format!("retention would not start: {error}")))?;
    eprintln!(
        "oqueue: WARNING -- consumer-group membership/generation is in memory (M4.15 owns the \
         durable one, ADR-0034). Group membership and generation do not survive a restart."
    );
    let cluster = oqueue_broker::Cluster::new(
        host,
        port,
        oqueue_broker::Sequencing::new(coordinator, reader),
        oqueue_broker::Seams {
            store,
            group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
            group_metadata_log,
        },
        &writer,
    )
    .await
    .map_err(|error| std::io::Error::other(format!("the writer identity was refused: {error}")))?;
    Ok(Built {
        cluster,
        serving,
        retention,
        log: current,
        lease,
    })
}

/// This node's handle on the shard's lease, tried once now so a healthy boot
/// leads at once; the keeper `serve` spawns retries until it holds the lease
/// and renews it after (`M6.7`, `M6.10`).
async fn lease_for(
    store: &Arc<dyn ObjectStore>,
    writer: &oqueue_broker::WriterId,
    clock: &Arc<dyn oqueue_core::Clock>,
) -> Arc<oqueue_core::ObjectStoreLease> {
    let lease = Arc::new(oqueue_core::ObjectStoreLease::new(
        Arc::clone(store),
        "meta/0",
        writer.as_str(),
        Arc::clone(clock),
    ));
    let _ = lease.acquire().await;
    lease
}

/// Both logs over `store`, opened on first use (`ADR-0046`, `M6.10`).
fn deferred_logs(
    store: &Arc<dyn ObjectStore>,
) -> (
    Arc<oqueue_core::ObjectStoreMetadataLog>,
    Arc<dyn oqueue_core::GroupMetadataLog>,
) {
    (
        Arc::new(oqueue_core::ObjectStoreMetadataLog::deferred(
            Arc::clone(store),
            "meta/0",
        )),
        Arc::new(oqueue_core::ObjectStoreGroupMetadataLog::deferred(
            Arc::clone(store),
            "groups/0",
        )),
    )
}

/// Opens a current view of the metadata log each time the coordinator starts
/// leading, and publishes it for the checkpoint cadence (`M6.17`).
fn reopener(
    store: &Arc<dyn ObjectStore>,
    current: &oqueue_broker::CurrentLog,
) -> oqueue_coordinator::Reopen {
    let (store, current) = (Arc::clone(store), Arc::clone(current));
    Box::new(move || {
        let (store, current) = (Arc::clone(&store), Arc::clone(&current));
        Box::pin(async move {
            let log = Arc::new(oqueue_core::ObjectStoreMetadataLog::open(store, "meta/0").await?);
            *current
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::clone(&log);
            Ok(log as Arc<dyn oqueue_core::MetadataLog>)
        })
    })
}
