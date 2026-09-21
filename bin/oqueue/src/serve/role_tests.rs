#![allow(clippy::expect_used)]

use super::spawn_role_tasks;
use crate::ProcessRole;
use std::sync::Arc;

#[tokio::test]
async fn role_task_spawner_requires_coordination_lease_and_returns_serving_handle() {
    let store: Arc<dyn crate::compose::Backend> = Arc::new(oqueue_core::FakeObjectStore::new());
    let coordinator = crate::compose::build_cluster(
        "h".to_owned(),
        1,
        Arc::clone(&store),
        oqueue_broker::NodeRole::Coordinator,
    )
    .await
    .expect("an empty coordinator fixture composes");
    let error = spawn_role_tasks(
        None,
        coordinator.retention,
        coordinator.log,
        None,
        ProcessRole::Coordinator,
    )
    .expect_err("a coordinator without a lease must not start");
    assert_eq!(
        error.to_string(),
        "coordinator role started without its lease"
    );

    let data_plane =
        crate::compose::build_cluster("h".to_owned(), 1, store, oqueue_broker::NodeRole::DataPlane)
            .await
            .expect("an empty data-plane fixture composes");
    let serving = spawn_role_tasks(
        data_plane.serving,
        data_plane.retention,
        data_plane.log,
        None,
        ProcessRole::DataPlane,
    )
    .expect("data-plane task setup does not need a lease")
    .expect("data-plane metadata replay must be supervised");
    serving.abort();
}
