//! One flush, one object: what FR-32's cost model rests on.
//!
//! `M3.13`. Doc 12 prices a PUT far above the bytes in it, so a broker writing
//! one object per topic would pay per topic for a workload whose cost is meant
//! to scale with bytes. What is pinned here is that a flush spanning N topics
//! produces **one** payload and **one** PUT, that the footer describing it can
//! be read back exactly, and that a torn or hostile tail is refused rather than
//! indexed past.

// Every `expect` is on a value the test built from a literal it controls.
#![allow(clippy::expect_used)]

use oqueue_core::{
    BundleBuilder, BundleNamer, ByteRange, CountingObjectStore, Error, FakeObjectStore, ObjectKey,
    ObjectStore, Operation, PartitionId, TopicId, parse_footer,
};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion. Everything under test is a synchronous fake,
/// so a `Pending` is transient and a spin always terminates — the same shape
/// `store.rs`'s own suite uses.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    loop {
        if let Poll::Ready(value) = Pin::new(&mut future).poll(&mut context) {
            return value;
        }
    }
}

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

/// A flush spanning four topics.
fn four_topics() -> BundleBuilder {
    let mut bundle = BundleBuilder::new();
    for (n, name) in ["orders", "payments", "shipments", "returns"]
        .into_iter()
        .enumerate()
    {
        let n = u32::try_from(n).expect("four fits");
        bundle
            .push(
                topic(name),
                partition(0),
                n + 1,
                &vec![b'a' + u8::try_from(n).expect("four fits"); 16 + n as usize],
            )
            .expect("a non-empty region");
    }
    bundle
}

/// ⚠️ **FR-32, and the reason the cost model works.** Four topics' records
/// become **one** payload, so writing them is one store call where the
/// unbundled shape is four.
///
/// ⚠️ **The contrast is the assertion.** A test that put one bundle and
/// asserted the count was one would be counting its own call and would pass
/// against any format at all, including a broken one — what makes this
/// constrain `BundleBuilder` is that the same four topics, written the way a
/// broker without this format would write them, cost four.
///
/// ⚠️ **And the count alone is not the assertion either.** One PUT that
/// carried one topic's records would satisfy a call count while losing three
/// topics, so the object is read back out of the store and its footer parsed:
/// the claim is that *four topics* are in the *one* object, which is what
/// costs one PUT.
///
/// ⚠️ **What this does *not* establish is the wire-level claim.** No flush
/// exists yet: nothing outside a test composes a builder, a store and a
/// coordinator. That composition is `M3.14`'s, and the row says so.
#[test]
fn bundling_four_topics_costs_one_store_call_where_four_objects_cost_four() {
    let sealed = four_topics().seal().expect("regions were pushed");
    assert_eq!(sealed.regions().len(), 4);
    assert_eq!(sealed.spans().len(), 4, "one span per region, to commit");

    let bundled = CountingObjectStore::new(FakeObjectStore::new());
    let key = ObjectKey::new("bundle-0".to_owned()).expect("a valid key");
    let payload = sealed.into_payload();
    block_on(bundled.put(&key, payload, None)).expect("the object lands");

    // ⚠️ Read back from the store, not from `sealed`: what a later reader gets
    // is the bytes, and a `seal` that described only the first region would
    // pass every assertion above.
    let stored = block_on(bundled.get(&key, ByteRange::Full)).expect("the object is there");
    let size = u64::try_from(stored.len()).expect("a small object");
    let regions = parse_footer(&stored, size).expect("its own footer parses");
    let names: Vec<&str> = regions.iter().map(|r| r.topic().as_str()).collect();
    assert_eq!(
        names,
        ["orders", "payments", "shipments", "returns"],
        "one PUT, and all four topics are inside it"
    );
    for (n, region) in regions.iter().enumerate() {
        let ByteRange::Bounded(bounds) = region.bytes() else {
            panic!("a region is always bounded");
        };
        let at = usize::try_from(bounds.offset()).expect("a small object");
        let len = usize::try_from(bounds.length()).expect("a small object");
        let want = vec![b'a' + u8::try_from(n).expect("four fits"); 16 + n];
        assert_eq!(&stored[at..at + len], &want[..], "region {n}'s own bytes");
    }

    // The shape this format exists to replace: one object per topic.
    let unbundled = CountingObjectStore::new(FakeObjectStore::new());
    for (n, name) in ["orders", "payments", "shipments", "returns"]
        .into_iter()
        .enumerate()
    {
        let mut one = BundleBuilder::new();
        one.push(topic(name), partition(0), 1, b"records")
            .expect("a non-empty region");
        let key = ObjectKey::new(format!("solo-{n}")).expect("a valid key");
        let payload = one.seal().expect("a region was pushed").into_payload();
        block_on(unbundled.put(&key, payload, None)).expect("the object lands");
    }

    assert_eq!(bundled.counts().count(Operation::Put), 1);
    assert_eq!(unbundled.counts().count(Operation::Put), 4);
    assert!(
        bundled.counts().count(Operation::Put) * 4 == unbundled.counts().count(Operation::Put),
        "four topics cost one call bundled and four unbundled — doc 12 prices \
         a PUT far above the bytes in it, which is why that ratio is the whole \
         cost model"
    );
}

/// Every region's range is disjoint, in order, and bounded — never
/// `ByteRange::Full`, which two regions of one object both claiming would have
/// a reader decode another topic's records as its own.
#[test]
fn regions_carve_the_payload_into_disjoint_bounded_ranges() {
    let sealed = four_topics().seal().expect("regions were pushed");

    let mut expected_offset = 0;
    for region in sealed.regions() {
        let ByteRange::Bounded(bounds) = region.bytes() else {
            panic!("a bundled region is never ByteRange::Full");
        };
        assert_eq!(bounds.offset(), expected_offset, "regions abut in order");
        assert!(bounds.length() > 0);
        expected_offset += bounds.length();
    }
    assert!(
        usize::try_from(expected_offset).expect("a small payload") < sealed.payload().len(),
        "the payload carries the regions and then the footer"
    );
}

/// The bytes a region names hold that region's records and no others.
#[test]
fn a_region_names_the_bytes_that_were_pushed_for_it() {
    let mut bundle = BundleBuilder::new();
    bundle
        .push(topic("orders"), partition(0), 1, b"orders-records")
        .expect("a non-empty region");
    bundle
        .push(topic("payments"), partition(3), 1, b"payments")
        .expect("a non-empty region");
    let sealed = bundle.seal().expect("regions were pushed");

    for (region, expected) in sealed
        .regions()
        .iter()
        .zip([b"orders-records".as_slice(), b"payments".as_slice()])
    {
        let ByteRange::Bounded(bounds) = region.bytes() else {
            panic!("bounded");
        };
        let at = usize::try_from(bounds.offset()).expect("a small payload");
        let len = usize::try_from(bounds.length()).expect("a small region");
        assert_eq!(&sealed.payload()[at..at + len], expected);
    }
}

/// The builder says how much it is holding, which is what a flush trigger
/// reads to decide whether to seal.
#[test]
fn the_builder_reports_how_many_regions_it_holds() {
    let mut bundle = BundleBuilder::new();
    assert!(bundle.is_empty());
    assert_eq!(bundle.len(), 0);

    for (n, name) in ["orders", "payments", "shipments"].into_iter().enumerate() {
        bundle
            .push(topic(name), partition(0), 1, b"records")
            .expect("a non-empty region");
        assert_eq!(bundle.len(), n + 1);
        assert!(!bundle.is_empty());
    }
}

/// ⚠️ A region's record count is what the coordinator folds into an offset, so
/// it has to survive the footer exactly — a count read back wrong moves every
/// later offset in that partition.
#[test]
fn record_counts_survive_the_footer_exactly() {
    let mut bundle = BundleBuilder::new();
    for (name, count) in [("orders", 1_u32), ("payments", 9), ("shipments", 4096)] {
        bundle
            .push(topic(name), partition(0), count, b"records")
            .expect("a non-empty region");
    }
    let sealed = bundle.seal().expect("regions were pushed");

    let counts: Vec<u32> = sealed
        .regions()
        .iter()
        .map(oqueue_core::Region::record_count)
        .collect();
    assert_eq!(counts, vec![1, 9, 4096]);

    let read =
        parse_footer(sealed.payload(), sealed.payload().len() as u64).expect("the footer parses");
    assert_eq!(
        read.iter()
            .map(oqueue_core::Region::record_count)
            .collect::<Vec<u32>>(),
        counts,
        "and the spans the coordinator commits agree with them"
    );
    assert_eq!(
        sealed
            .spans()
            .iter()
            .map(oqueue_core::CommittedSpan::record_count)
            .collect::<Vec<u32>>(),
        counts
    );
}

/// ⚠️ A topic name too long for the footer's length field is refused **at
/// push**, before any bytes are copied — a truncated name would have a reader
/// parse a different topic's records under a name it recognises.
#[test]
fn a_topic_name_too_long_for_the_footer_is_refused() {
    let mut bundle = BundleBuilder::new();
    let longest = "t".repeat(usize::from(u16::MAX));
    bundle
        .push(topic(&longest), partition(0), 1, b"records")
        .expect("exactly the longest name the field holds is fine");

    let too_long = "t".repeat(usize::from(u16::MAX) + 1);
    assert_eq!(
        bundle
            .push(topic(&too_long), partition(0), 1, b"records")
            .err(),
        Some(Error::BundleTooLarge)
    );
    assert_eq!(bundle.len(), 1, "and the over-long region was not added");
}

/// An empty flush is refused: the PUT would cost what a full one costs and
/// carry nothing, and committing no spans burns a version on a record saying
/// nothing happened.
#[test]
fn an_empty_bundle_is_refused_rather_than_written() {
    assert!(BundleBuilder::new().is_empty());
    assert_eq!(BundleBuilder::new().seal().err(), Some(Error::EmptyBundle));
}

/// A region with no bytes, or no records, is refused at the point it is pushed.
///
/// ⚠️ **Both halves, and the second is the one that would be silent.** A
/// zero-length range never reaches a footer for a reader to trip over; a region
/// carrying bytes under a count of zero would be durable, indexed, billed and
/// unreachable, because the fold turns counts into offsets and zero advances
/// none.
#[test]
fn a_region_with_no_bytes_or_no_records_is_refused_at_push() {
    let mut bundle = BundleBuilder::new();
    assert_eq!(
        bundle.push(topic("orders"), partition(0), 1, b"").err(),
        Some(Error::EmptyByteRange)
    );
    assert_eq!(
        bundle
            .push(topic("orders"), partition(0), 0, b"records")
            .err(),
        Some(Error::EmptyRegion)
    );
    assert!(bundle.is_empty(), "and nothing was added");
}

/// ⚠️ **The naming `ADR-0026`'s decision rests on.** Writing a bundle without a
/// `Precondition` is safe only if the key is one nothing else writes; a
/// monotonic sequence per writer is what makes that true, in doc 12 §4.6's
/// shape.
#[test]
fn one_writer_never_names_two_objects_alike() {
    let mut namer = BundleNamer::new("writer-a").expect("a valid identity");
    let keys: Vec<ObjectKey> = (0..64).map(|_| namer.next_key().expect("a key")).collect();

    let mut unique: Vec<&str> = keys.iter().map(ObjectKey::as_str).collect();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), keys.len(), "every flush got its own name");
}

/// And two writers never collide either — which is the half that rests on the
/// composer minting distinct identities.
#[test]
fn two_writers_never_name_an_object_alike() {
    let mut a = BundleNamer::new("writer-a").expect("a valid identity");
    let mut b = BundleNamer::new("writer-b").expect("a valid identity");

    for _ in 0..32 {
        assert_ne!(
            a.next_key().expect("a key").as_str(),
            b.next_key().expect("a key").as_str()
        );
    }
}

/// ⚠️ A `/` in a writer identity would let two distinct identities produce one
/// key by moving the boundary between the writer and its sequence — so it is
/// refused where the identity is built, not where the collision would show up.
#[test]
fn a_writer_identity_that_could_forge_a_key_is_refused() {
    assert_eq!(
        BundleNamer::new("a/b").err(),
        Some(Error::MalformedWriterId)
    );
    assert_eq!(BundleNamer::new("").err(), Some(Error::EmptyObjectKey));
    assert!(BundleNamer::new("writer-a").is_ok());
}
