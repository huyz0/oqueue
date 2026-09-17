//! The merge executor's happy paths, against the fake store.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::merge;
use oqueue_core::{BundleBuilder, ByteRange, ObjectRef, ObjectStore, PushedRecords, parse_footer};
use std::sync::atomic::Ordering;

use crate::support::{Counting, inputs_of, key, offset, other, partition, planned, topic};

#[tokio::test]
async fn a_merge_reads_each_input_exactly_once() {
    let store = Counting::new();
    let refs = inputs_of(&store, 13, 1, 64).await;
    let outcome = merge(&store, &planned(13), &refs, &key("out"))
        .await
        .expect("a merge that runs");

    assert_eq!(
        outcome.gets(),
        refs.len(),
        "one read per input, never a re-read for the footer"
    );
    assert_eq!(store.gets.load(Ordering::Relaxed), refs.len());
    assert_eq!(outcome.puts(), 1, "one output object");
    assert_eq!(outcome.records(), 13);
}

/// ⚠️ **Sequential, so at most one input's bytes are resident.** A
/// prefetching merge would hold as many inputs as it had in flight.
#[tokio::test]
async fn a_merge_holds_one_input_at_a_time() {
    let store = Counting::new();
    let refs = inputs_of(&store, 50, 1, 1024).await;
    merge(&store, &planned(50), &refs, &key("out"))
        .await
        .expect("a merge that runs");

    assert_eq!(
        store.peak_in_flight.load(Ordering::Relaxed),
        1,
        "one read in flight at a time is what bounds the input side"
    );
    assert!(
        store.peak_bytes.load(Ordering::Relaxed) < 2048,
        "no read ever returns more than one input"
    );
}

/// ⚠️ **Offset order is the merge's responsibility, not the caller's.** An
/// output whose regions are out of order parses perfectly and serves one
/// offset's records for another's.
#[tokio::test]
async fn inputs_are_merged_in_offset_order_whatever_order_they_arrive_in() {
    let store = Counting::new();
    let markers: Vec<u8> = (b'a'..=b'm').collect();
    let mut refs = Vec::new();
    for (i, marker) in markers.iter().enumerate() {
        let name = format!("in-{i}");
        let mut builder = BundleBuilder::new();
        builder
            .push(
                topic(),
                partition(),
                PushedRecords {
                    count: 1,
                    producer: None,
                },
                &[*marker; 8],
            )
            .expect("a valid region");
        let sealed = builder.seal().expect("a sealed bundle");
        store
            .inner
            .put(&key(&name), sealed.into_payload(), None)
            .await
            .expect("a store that accepts");
        refs.push(ObjectRef::new(
            key(&name),
            offset(i64::try_from(i).expect("a small count")),
            1,
        ));
    }
    refs.reverse();

    merge(&store, &planned(13), &refs, &key("out"))
        .await
        .expect("a merge that runs");

    let written = store
        .inner
        .get(&key("out"), ByteRange::Full)
        .await
        .expect("an output object");
    let expected: Vec<u8> = markers
        .iter()
        .flat_map(|marker| std::iter::repeat_n(*marker, 8))
        .collect();
    assert_eq!(
        &written[..expected.len()],
        &expected[..],
        "ascending base offset, whatever order the refs arrived in"
    );
}

/// The output must hold this partition's records and nobody else's.
#[tokio::test]
async fn a_merge_takes_only_the_planned_partition_s_regions() {
    let store = Counting::new();
    let mut refs = inputs_of(&store, 12, 1, 32).await;

    // A thirteenth object bundling two topics, as FR-32's flush does.
    let mut builder = BundleBuilder::new();
    builder
        .push(
            other(),
            partition(),
            PushedRecords {
                count: 7,
                producer: None,
            },
            &[b'x'; 32],
        )
        .expect("a valid region");
    builder
        .push(
            topic(),
            partition(),
            PushedRecords {
                count: 1,
                producer: None,
            },
            &[b'r'; 32],
        )
        .expect("a valid region");
    let sealed = builder.seal().expect("a sealed bundle");
    store
        .inner
        .put(&key("mixed"), sealed.into_payload(), None)
        .await
        .expect("a store that accepts");
    refs.push(ObjectRef::new(key("mixed"), offset(12), 1));

    let outcome = merge(&store, &planned(13), &refs, &key("out"))
        .await
        .expect("a merge that runs");
    assert_eq!(
        outcome.records(),
        13,
        "the other topic's seven records stay behind"
    );

    let written = store
        .inner
        .get(&key("out"), ByteRange::Full)
        .await
        .expect("an output object");
    let regions = parse_footer(&written, written.len() as u64).expect("a valid footer");
    assert_eq!(regions.len(), 13);
    assert!(regions.iter().all(|region| region.topic() == &topic()));
}
