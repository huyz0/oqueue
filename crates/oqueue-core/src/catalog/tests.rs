//! `TopicCatalog`'s contract, as cases any implementation can be run against.

#![allow(clippy::expect_used)]

use super::{CatalogEntry, FakeTopicCatalog, TopicCatalog, TopicDeleteOutcome, topic_uuid};
use crate::test_executor::block_on;
use crate::{Principal, TopicId};

fn topic(name: &str) -> TopicId {
    TopicId::new(name).expect("a topic")
}

/// Guarantee 1: creation is idempotent and never overwrites.
pub(super) fn create_is_idempotent(catalog: &dyn TopicCatalog) {
    let first = block_on(catalog.create(&topic("orders"), 3)).expect("creates");
    let again = block_on(catalog.create(&topic("orders"), 9)).expect("answers");
    assert_eq!(
        first, again,
        "the existing entry, not a new partition count"
    );
    assert_eq!(first.partitions(), 3);
    assert_eq!(
        block_on(catalog.lookup(&topic("orders"))).expect("reads"),
        Some(first)
    );
    assert_eq!(
        block_on(catalog.lookup(&topic("absent"))).expect("reads"),
        None
    );
}

/// Guarantee 3: an id finds what a name finds.
pub(super) fn lookup_id_agrees_with_lookup(catalog: &dyn TopicCatalog) {
    let entry = block_on(catalog.create(&topic("payments"), 1)).expect("creates");
    assert_eq!(
        block_on(catalog.lookup_id(entry.id())).expect("reads"),
        Some(entry)
    );
    assert_eq!(block_on(catalog.lookup_id(7)).expect("reads"), None);
}

/// Guarantee 4: listing pages in name order.
pub(super) fn list_pages_in_name_order(catalog: &dyn TopicCatalog) {
    for name in ["c", "a", "d", "b"] {
        block_on(catalog.create(&topic(name), 1)).expect("creates");
    }
    let first = block_on(catalog.list(None, 2)).expect("lists");
    assert_eq!(first, vec![topic("a"), topic("b")]);
    let rest = block_on(catalog.list(first.last(), 10)).expect("lists");
    assert_eq!(rest, vec![topic("c"), topic("d")]);
    assert!(
        block_on(catalog.list(Some(&topic("d")), 10))
            .expect("lists")
            .is_empty()
    );
}

/// Owned creation is durable, idempotent, and scoped to the winner.
pub(super) fn owned_creation_is_scoped(catalog: &dyn TopicCatalog) {
    let alice = Principal::new("alice").expect("principal");
    let bob = Principal::new("bob").expect("principal");
    let first = block_on(catalog.create_owned(&topic("owned"), 2, &alice)).expect("creates");
    assert!(first.created());
    assert_eq!(first.entry().creator(), Some(&alice));
    let again = block_on(catalog.create_owned(&topic("owned"), 9, &bob)).expect("answers");
    assert!(!again.created());
    assert_eq!(again.entry(), first.entry());
    assert_eq!(
        block_on(catalog.list_owned(&alice, None, 10)).expect("lists"),
        vec![topic("owned")]
    );
    assert!(
        block_on(catalog.list_owned(&bob, None, 10))
            .expect("lists")
            .is_empty()
    );
}

/// Deletion is durable at the seam: it hides the live row, rejects a stale
/// UUID, and reserves the name forever.
pub(super) fn deletion_is_durable_and_non_reusable(catalog: &dyn TopicCatalog) {
    let name = topic("retired");
    let entry = block_on(catalog.create(&name, 3)).expect("creates");
    assert!(matches!(
        block_on(catalog.delete(&name, Some(entry.id()))).expect("deletes"),
        TopicDeleteOutcome::Deleted(deleted) if deleted == entry
    ));
    assert_eq!(block_on(catalog.lookup(&name)).expect("reads"), None);
    assert_eq!(
        block_on(catalog.lookup_id(entry.id())).expect("reads"),
        None
    );
    assert!(block_on(catalog.list(None, 10)).expect("lists").is_empty());
    assert!(matches!(
        block_on(catalog.delete(&name, Some(entry.id()))).expect("idempotent delete"),
        TopicDeleteOutcome::AlreadyDeleted { id } if id == entry.id()
    ));
    assert!(matches!(
        block_on(catalog.create(&name, 1)),
        Err(crate::Error::TopicNameReserved)
    ));
}

pub(super) fn deletion_rejects_a_stale_uuid(catalog: &dyn TopicCatalog) {
    let name = topic("orders");
    let entry = block_on(catalog.create(&name, 1)).expect("creates");
    assert!(matches!(
        block_on(catalog.delete(&name, Some(entry.id() ^ 1))).expect("checks uuid"),
        TopicDeleteOutcome::StaleId { expected, actual }
            if expected == (entry.id() ^ 1) && actual == entry.id()
    ));
    assert_eq!(
        block_on(catalog.lookup(&name)).expect("still live"),
        Some(entry)
    );
}

#[test]
fn the_fake_keeps_the_contract() {
    create_is_idempotent(&FakeTopicCatalog::new());
    lookup_id_agrees_with_lookup(&FakeTopicCatalog::new());
    list_pages_in_name_order(&FakeTopicCatalog::new());
    owned_creation_is_scoped(&FakeTopicCatalog::new());
    deletion_is_durable_and_non_reusable(&FakeTopicCatalog::new());
    deletion_rejects_a_stale_uuid(&FakeTopicCatalog::new());
}

#[test]
fn a_topic_id_is_derived_from_the_name_and_is_a_valid_uuid() {
    let id = topic_uuid(&topic("orders"));
    assert_eq!(id, topic_uuid(&topic("orders")), "stable");
    assert_ne!(id, topic_uuid(&topic("orderz")), "one byte moves it");
    assert_ne!(id, 0, "never nil");
    assert_eq!((id >> 76) & 0xf, 8, "version 8");
    assert_eq!((id >> 62) & 0x3, 2, "RFC 4122 variant");
    assert_eq!(CatalogEntry::new(topic("orders"), 1).id(), id);
}
