//! `TopicCatalog`'s contract, as cases any implementation can be run against.

#![allow(clippy::expect_used)]

use super::{CatalogEntry, FakeTopicCatalog, TopicCatalog, topic_uuid};
use crate::TopicId;
use crate::test_executor::block_on;

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

#[test]
fn the_fake_keeps_the_contract() {
    create_is_idempotent(&FakeTopicCatalog::new());
    lookup_id_agrees_with_lookup(&FakeTopicCatalog::new());
    list_pages_in_name_order(&FakeTopicCatalog::new());
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
