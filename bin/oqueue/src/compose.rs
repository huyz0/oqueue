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
/// ⚠️ **`FakeMetadataLog` is the only `MetadataLog` `M3` builds**
/// (`ADR-0020` point 5, doc 10 #12, `M3.md`'s goal note), so this broker's
/// offsets do not survive its process: a restart re-bases at `Offset::ZERO`
/// over objects that already hold those offsets. `roadmap.md`'s deferral table
/// gives the durable engine to `M6`. The warning below is so an operator hears
/// it from the broker rather than from a consumer.
///
/// ⚠️ **The coordinator's loop is returned, not spawned here.** It is spawned
/// by `serve`, which watches its handle — `async-concurrency.md` rule 13 wants
/// an owner that can *observe* the task, and a `JoinHandle` dropped on the
/// floor observes nothing. A loop that panicked would otherwise leave a process
/// that stays up, accepts connections, and answers every produce
/// `LEADER_NOT_AVAILABLE` forever while no supervisor restarts it, because it
/// never exits and says nothing.
pub(crate) async fn build_cluster(
    host: String,
    port: i32,
    store: Arc<dyn ObjectStore>,
) -> std::io::Result<(
    oqueue_broker::Cluster,
    oqueue_coordinator::CoordinatorLoop,
    oqueue_broker::Retention,
)> {
    let log: Arc<dyn oqueue_core::MetadataLog> = Arc::new(oqueue_core::FakeMetadataLog::new());
    let clock: Arc<dyn oqueue_core::Clock> = Arc::new(crate::wall_clock::WallClock);
    let index = Box::new(oqueue_index::MemoryIndex::new());
    let epoch = oqueue_core::CoordinatorEpoch::new(1);
    let (coordinator, serving, reader) =
        oqueue_coordinator::Coordinator::open(Arc::clone(&log), index, epoch, Arc::clone(&clock))
            .await
            .map_err(|error| {
                std::io::Error::other(format!("the coordinator would not open: {error}"))
            })?;
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
        "oqueue: WARNING -- the metadata log is in memory (M6 owns the durable one). \
         Offsets do not survive a restart."
    );
    eprintln!(
        "oqueue: WARNING -- consumer-group membership/generation is in memory (M4.15 owns the \
         durable one, ADR-0034). Group membership and generation do not survive a restart."
    );
    eprintln!(
        "oqueue: WARNING -- the group metadata log is in memory (M6 owns the durable engine, \
         ADR-0035, the same one M4.14 names for the topic metadata log above). Committed \
         offsets replay correctly across an in-process restart but do not survive the process."
    );
    let cluster = oqueue_broker::Cluster::new(
        host,
        port,
        oqueue_broker::Sequencing::new(coordinator, reader),
        oqueue_broker::Seams {
            store,
            group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
            group_metadata_log: Arc::new(oqueue_core::FakeGroupMetadataLog::new()),
        },
        &oqueue_broker::WriterId::mint(),
    )
    .await
    .map_err(|error| std::io::Error::other(format!("the writer identity was refused: {error}")))?;
    Ok((cluster, serving, retention))
}
