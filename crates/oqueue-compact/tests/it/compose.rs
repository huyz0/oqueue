//! Composing objects: the compaction that moves no records.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::{compose, read_composite};
use oqueue_core::{ByteRange, ObjectStore, locate, parse_footer};
use std::sync::atomic::Ordering;

use crate::support::{Counting, inputs_of, key, partition, topic};

/// ⚠️ **`M5.8`'s acceptance, and the whole point of a composite**: N objects
/// become one from the coordinator's view, every record byte stays where it
/// was, and a fetch through the composite reads the same bytes it read before.
#[tokio::test]
async fn composing_moves_no_records() {
    let store = Counting::new();
    let count = 12_usize;
    let refs = inputs_of(&store, count, 3, 64).await;
    let before: u64 = refs.len() as u64;
    store.puts.store(0, Ordering::Relaxed);

    let outcome = compose(&store, &refs, &key("composite"))
        .await
        .expect("a composite");
    assert_eq!(outcome.components(), count, "every component is named");
    assert_eq!(outcome.gets(), count, "one footer read per component");
    assert_eq!(
        store.puts.load(Ordering::Relaxed),
        1,
        "one PUT, and it is the manifest"
    );

    // ⚠️ **The claim the row asks a test to observe.** Twelve objects of three
    // 64-byte records each hold 2304 record bytes between them; the manifest
    // is a few hundred, and what makes that true is that `compose` never sees
    // a record byte to write.
    let records = count * 3 * 64;
    assert!(
        outcome.manifest_bytes() < records,
        "the manifest is smaller than the records it names: {} against {records}",
        outcome.manifest_bytes()
    );
    // ⚠️ **And it is the object's own length**, which is what makes the number
    // a measurement rather than a claim: a `manifest_bytes` of zero, or of
    // anything else, disagrees with what the store holds.
    let written = store
        .inner
        .get(&key("composite"), ByteRange::Full)
        .await
        .expect("the composite object");
    assert_eq!(
        outcome.manifest_bytes(),
        written.len(),
        "the bytes it reports are the bytes it wrote"
    );

    // The coordinator's object count: one entry where there were twelve.
    assert_eq!(before, 12, "twelve objects before");
    let components = read_composite(&store, &key("composite"))
        .await
        .expect("a manifest that reads back");
    assert_eq!(components.len(), count);
}

/// ⚠️ **The other half of the acceptance, and its own test**: a fetch through
/// the composite lands on the same component bytes a direct read does. The
/// two halves are one run of `compose` each because the first is about what
/// was written and this is about what can be read back.
#[tokio::test]
async fn a_fetch_through_a_composite_finds_the_same_bytes() {
    let store = Counting::new();
    let count = 12_usize;
    let refs = inputs_of(&store, count, 3, 64).await;
    compose(&store, &refs, &key("composite"))
        .await
        .expect("a composite");
    let components = read_composite(&store, &key("composite"))
        .await
        .expect("a manifest that reads back");

    let found = locate(&components, &topic(), partition());
    assert_eq!(found.len(), count, "one run per component");
    for (which, located) in found.iter().enumerate() {
        let whole = store
            .inner
            .get(located.object(), ByteRange::Full)
            .await
            .expect("the component");
        let regions = parse_footer(&whole, whole.len() as u64).expect("a valid footer");
        let ByteRange::Bounded(direct) = regions[0].bytes() else {
            panic!("a bundled region is always bounded")
        };
        let ByteRange::Bounded(through) = located.bytes() else {
            panic!("a composite names bounded ranges")
        };
        assert_eq!(
            (through.offset(), through.length()),
            (direct.offset(), direct.length()),
            "component {which} is described identically either way"
        );
        assert_eq!(located.record_count(), regions[0].record_count());
    }
}

/// A composite of nothing is refused rather than written.
#[tokio::test]
async fn composing_nothing_is_refused() {
    let store = Counting::new();
    compose(&store, &[], &key("empty"))
        .await
        .expect_err("a composite naming nothing is refused rather than written");
    assert_eq!(
        store.puts.load(Ordering::Relaxed),
        0,
        "and nothing was written"
    );
}

/// ⚠️ **The composite's form of the hazard `merge`'s `tiling()` refuses by
/// name.** A manifest naming one object twice resolves to two runs over the
/// same bytes, so a fetch counts those records twice and a retention pass
/// believes the object holds twice what it does — and nothing downstream can
/// tell that from an object genuinely holding two slices.
#[tokio::test]
async fn a_component_named_twice_is_refused() {
    let store = Counting::new();
    let refs = inputs_of(&store, 3, 3, 64).await;
    let doubled = vec![refs[0].clone(), refs[0].clone(), refs[1].clone()];
    store.puts.store(0, Ordering::Relaxed);

    compose(&store, &doubled, &key("composite"))
        .await
        .expect_err("one object named twice is refused");
    assert_eq!(
        store.puts.load(Ordering::Relaxed),
        0,
        "and nothing was written"
    );
}

/// ⚠️ **The manifest's order is the objects' order, not the caller's.**
/// `locate` returns a partition's runs in manifest order, so a caller's list
/// order would decide what order a fetch reads its own records in.
#[tokio::test]
async fn components_are_ordered_by_offset_whatever_order_they_arrive_in() {
    let store = Counting::new();
    let mut refs = inputs_of(&store, 4, 3, 64).await;
    refs.reverse();

    compose(&store, &refs, &key("composite"))
        .await
        .expect("a composite");
    let components = read_composite(&store, &key("composite"))
        .await
        .expect("a manifest that reads back");
    let names: Vec<&str> = components
        .iter()
        .map(|component| component.object().as_str())
        .collect();
    assert_eq!(
        names,
        vec!["in-0", "in-1", "in-2", "in-3"],
        "ascending by base offset, although they arrived reversed"
    );
}
