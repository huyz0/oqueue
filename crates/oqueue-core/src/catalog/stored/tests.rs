//! `ObjectStoreTopicCatalog` against the catalog's contract, and the store.

#![allow(clippy::expect_used)]

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use super::super::tests::{
    create_is_idempotent, list_pages_in_name_order, lookup_id_agrees_with_lookup,
};
use super::{ObjectStoreTopicCatalog, decode, encode, hex};
use crate::test_executor::block_on;
use crate::{
    CountingObjectStore, Error, FakeObjectStore, KeyDomain, KeyId, MetadataShardId, ObjectKey,
    ObjectStore, Operation, Principal, TopicCatalog, TopicId,
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
fn race_retry_yields_to_the_creator_task() {
    let mut future = Box::pin(super::cooperative_yield());
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
    assert!(matches!(
        future.as_mut().poll(&mut context),
        Poll::Ready(())
    ));
}

#[test]
fn it_keeps_the_contract() {
    create_is_idempotent(&fresh());
    lookup_id_agrees_with_lookup(&fresh());
    list_pages_in_name_order(&fresh());
}

#[test]
fn owned_creation_survives_a_fresh_catalog_and_is_scoped() {
    let store = Arc::new(FakeObjectStore::new());
    let first = over(&store);
    let alice = Principal::new("alice").expect("principal");
    let bob = Principal::new("bob").expect("principal");
    let outcome = block_on(first.create_owned(&topic("orders"), 2, &alice)).expect("creates");
    assert!(outcome.created());
    let restarted = over(&store);
    assert_eq!(
        block_on(restarted.list_owned(&alice, None, 10)).expect("lists"),
        vec![topic("orders")]
    );
    assert!(
        block_on(restarted.list_owned(&bob, None, 10))
            .expect("lists")
            .is_empty()
    );
    assert!(
        store
            .keys()
            .iter()
            .any(|key| key.as_str().contains("/owner/")),
        "owned creation writes an owner index"
    );
    let duplicate = block_on(restarted.create_owned(&topic("orders"), 7, &bob)).expect("answers");
    assert!(!duplicate.created());
    assert_eq!(duplicate.entry().creator(), Some(&alice));
}

#[test]
fn owned_retry_repairs_a_topic_published_before_its_id_index() {
    let store = Arc::new(FakeObjectStore::new());
    let catalog = over(&store);
    let alice = Principal::new("alice").expect("principal");
    let name = topic("interrupted");
    let entry = crate::CatalogEntry::with_creator(name.clone(), 1, alice.clone());
    block_on(store.put(
        &catalog.owner_key(&alice, &name).expect("owner key"),
        Vec::new(),
        None,
    ))
    .expect("owner index");
    block_on(store.put(
        &catalog.topic_key(&name).expect("topic key"),
        encode(&entry).expect("entry bytes"),
        None,
    ))
    .expect("topic entry");

    assert_eq!(block_on(catalog.lookup(&name)).expect("lookup"), None);
    let retry = block_on(catalog.create_owned(&name, 9, &alice)).expect("repairs");
    assert!(!retry.created());
    assert_eq!(
        block_on(catalog.lookup(&name)).expect("lookup"),
        Some(entry.clone())
    );
    assert_eq!(
        block_on(catalog.lookup_id(entry.id())).expect("lookup id"),
        Some(entry)
    );
}

#[test]
fn owned_customer_entries_round_trip_through_the_private_wire_format() {
    let name = topic("private");
    let key = KeyId::new("customer-kek").expect("a key id");
    let creator = Principal::new("alice").expect("a principal");
    let expected = crate::CatalogEntry::with_key_domain_and_creator(
        name,
        7,
        KeyDomain::customer(key),
        creator,
    );
    let bytes = encode(&expected).expect("encodes");
    assert_eq!(decode(&bytes).expect("decodes"), expected);
}

#[test]
fn owned_listing_skips_stale_owner_keys_without_truncating_a_full_page() {
    let store = Arc::new(FakeObjectStore::new());
    let catalog = over(&store);
    let alice = Principal::new("alice").expect("principal");
    for number in 0..1_000 {
        block_on(catalog.create_owned(&topic(&format!("topic-{number:04}")), 1, &alice))
            .expect("creates");
    }
    let stale = catalog
        .owner_key(&alice, &topic("0000-stale"))
        .expect("stale owner key");
    block_on(store.put(&stale, vec![1], None)).expect("stale index");

    let names = block_on(catalog.list_owned(&alice, None, 1_000)).expect("lists");
    assert_eq!(names.len(), 1_000);
    assert!(!names.iter().any(|name| name.as_str() == "0000-stale"));
}

#[test]
fn owned_page_arithmetic_stops_only_at_the_requested_limit() {
    assert!(ObjectStoreTopicCatalog::owned_page_needs_more(999, 1_000));
    assert!(!ObjectStoreTopicCatalog::owned_page_needs_more(
        1_000, 1_000
    ));
    assert_eq!(ObjectStoreTopicCatalog::owned_page_remaining(1_000, 999), 1);
    assert_eq!(ObjectStoreTopicCatalog::owned_page_remaining(1_000, 998), 2);
    assert!(ObjectStoreTopicCatalog::owned_page_reached_limit(
        1_000, 1_000
    ));
    assert!(!ObjectStoreTopicCatalog::owned_page_reached_limit(
        999, 1_000
    ));
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
