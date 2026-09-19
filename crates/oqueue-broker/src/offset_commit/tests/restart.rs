//! Committed offsets across a restart of the whole broker (`M6.6`).
//!
//! ⚠️ **Split from `tests.rs` at the 500-line limit, along the concept**: that
//! file is the handler's answers; this is what outlives the process.

use super::{body, commit, open, seat};
use oqueue_core::{GroupMetadataLog, TopicId};
use std::sync::Arc;

/// ⚠️ **FR-21's own verification method** (`M6.6`): a consumer commits an
/// offset, the broker is gone, and a new one — rebuilt from the object store
/// alone, its group log opened afresh — reads the same offset back. The fake
/// log cannot pass this; `ObjectStoreGroupMetadataLog` is what makes it true.
#[tokio::test(start_paused = true)]
async fn committed_offsets_survive_a_restart() {
    use oqueue_core::{FakeObjectStore, ObjectStore, ObjectStoreGroupMetadataLog};
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let durable = || async {
        Arc::new(
            ObjectStoreGroupMetadataLog::open(Arc::clone(&store), "groups/0")
                .await
                .expect("the group log opens"),
        ) as Arc<dyn GroupMetadataLog>
    };

    let before =
        crate::testing::with_group_log(&["orders"], Arc::clone(&store), durable().await).await;
    seat(&before, "g", "m1");
    commit(&before, &body("g", 1, "m1", &[("orders", &[0])]), &open()).await;
    drop(before);

    let after =
        crate::testing::with_group_log(&["orders"], Arc::clone(&store), durable().await).await;
    let group = oqueue_core::GroupId::new("g").expect("valid");
    let orders = TopicId::new("orders").expect("valid");
    assert_eq!(after.committed_offsets().get(&group, &orders, 0), Some(42));
}
