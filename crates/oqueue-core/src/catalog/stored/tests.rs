//! `ObjectStoreTopicCatalog` against the catalog's contract, and the store.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use super::super::tests::{
    create_is_idempotent, list_pages_in_name_order, lookup_id_agrees_with_lookup,
};
use super::{ObjectStoreTopicCatalog, hex};
use crate::test_executor::block_on;
use crate::{
    CountingObjectStore, Error, FakeObjectStore, KeyDomain, KeyId, MetadataShardId, ObjectKey,
    ObjectStore, Operation, TopicCatalog, TopicId,
};

fn topic(name: &str) -> TopicId {
    TopicId::new(name).expect("a topic")
}

fn over(store: &Arc<FakeObjectStore>) -> ObjectStoreTopicCatalog {
    ObjectStoreTopicCatalog::new(
        Arc::clone(store) as _,
        Arc::clone(store) as _,
        MetadataShardId::ZERO,
    )
}

fn fresh() -> ObjectStoreTopicCatalog {
    over(&Arc::new(FakeObjectStore::new()))
}

#[test]
fn it_keeps_the_contract() {
    create_is_idempotent(&fresh());
    lookup_id_agrees_with_lookup(&fresh());
    list_pages_in_name_order(&fresh());
}

#[test]
fn two_catalogs_over_one_store_agree() {
    let store = Arc::new(FakeObjectStore::new());
    let (one, other) = (over(&store), over(&store));
    let created = block_on(one.create(&topic("orders"), 4)).expect("creates");
    assert_eq!(
        block_on(other.lookup(&topic("orders"))).expect("reads"),
        Some(created.clone())
    );
    assert_eq!(
        block_on(other.lookup_id(created.id())).expect("reads"),
        Some(created.clone())
    );
    assert_eq!(
        block_on(other.list(None, 10)).expect("lists"),
        vec![topic("orders")]
    );
    let again = block_on(other.create(&topic("orders"), 9)).expect("answers");
    assert_eq!(again, created, "the other's entry, not a second one");
}

#[test]
fn customer_key_domain_round_trips_without_changing_the_default_format() {
    let store = Arc::new(FakeObjectStore::new());
    let catalog = over(&store);
    let expected_domain = KeyDomain::customer(KeyId::new("customer-kek").expect("a key id"));
    let created =
        block_on(catalog.create_with_key_domain(&topic("private"), 2, expected_domain.clone()))
            .expect("creates");

    assert_eq!(created.key_domain(), &expected_domain);
    assert_eq!(
        block_on(catalog.lookup(&topic("private"))).expect("reads"),
        Some(created.clone())
    );
    assert_eq!(
        block_on(catalog.lookup_id(created.id())).expect("reads"),
        Some(created)
    );
}

#[test]
fn an_unrepresentable_customer_key_is_refused_before_catalog_encoding() {
    let err = KeyId::new("x".repeat(usize::from(u16::MAX) + 1))
        .expect_err("the key id validates before a catalog entry can hold it");

    assert_eq!(
        err,
        Error::InvalidKeyId {
            reason: "exceeds the maximum encoded length"
        }
    );
}

/// Guarantee 2, as far as `oqueue-core` can see it: it has no runtime to
/// spawn on, so "starts no task and opens no log" is asserted as "writes
/// exactly two objects under the catalog prefix, and nothing else".
#[test]
fn creation_provisions_nothing() {
    let counted = Arc::new(CountingObjectStore::new(FakeObjectStore::new()));
    let catalog = ObjectStoreTopicCatalog::new(
        Arc::clone(&counted) as _,
        Arc::new(FakeObjectStore::new()),
        MetadataShardId::ZERO,
    );
    block_on(catalog.create(&topic("orders"), 3)).expect("creates");
    assert_eq!(counted.counts().count(Operation::Put), 2);
    assert_eq!(counted.counts().count(Operation::Get), 0);
    assert_eq!(counted.counts().count(Operation::Delete), 0);
    let keys = counted.inner().keys();
    assert_eq!(keys.len(), 2);
    assert!(
        keys.iter()
            .all(|key| key.as_str().starts_with("catalog/0/")),
        "{keys:?}"
    );
}

#[test]
fn a_malformed_entry_is_refused_not_misread() {
    let store = Arc::new(FakeObjectStore::new());
    let catalog = over(&store);
    let key = ObjectKey::new(format!("catalog/0/topic/{}", hex(b"orders"))).expect("a key");
    for body in [
        vec![],
        vec![2, 0, 0, 0, 1, b'o'],
        vec![1, 0, 0],
        vec![1, 0, 0, 0, 1],
        vec![1, 0, 0, 0, 1, 0xff],
        [&[1, 0, 0, 0, 1][..], b"payments"].concat(),
    ] {
        block_on(store.put(&key, body.clone(), None)).expect("writes");
        assert!(
            matches!(
                block_on(catalog.lookup(&topic("orders"))),
                Err(Error::MalformedMetadataSegment { .. })
            ),
            "{body:?}"
        );
    }
}

#[test]
fn any_name_round_trips_and_lists_in_byte_order() {
    let catalog = fresh();
    let names = ["a/b", "a", "é", "zz", "a0", "日本"];
    for name in names {
        let entry = block_on(catalog.create(&topic(name), 2)).expect("creates");
        assert_eq!(
            block_on(catalog.lookup(&topic(name))).expect("reads"),
            Some(entry.clone())
        );
        assert_eq!(
            block_on(catalog.lookup_id(entry.id())).expect("reads"),
            Some(entry)
        );
    }
    let mut sorted: Vec<TopicId> = names.iter().map(|name| topic(name)).collect();
    sorted.sort();
    assert_eq!(block_on(catalog.list(None, 10)).expect("lists"), sorted);
    let after = block_on(catalog.list(Some(&topic("a/b")), 10)).expect("lists");
    assert_eq!(after, sorted[2..].to_vec());
}
