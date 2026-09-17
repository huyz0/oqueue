//! The composite manifest format: what it round-trips, and what it refuses.
//!
//! ⚠️ **The refusals are the subject.** These bytes come off the network, so
//! every one of them is a thing that happens rather than a thing that would
//! mean a bug — `security.md` rule 3.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{
    BundleBuilder, COMPOSITE_MAGIC, COMPOSITE_TRAILER_LEN, CompositeBuilder, Error, PartitionId,
    PushedRecords, TopicId, locate, parse_composite, parse_footer,
};

fn topic() -> TopicId {
    TopicId::new("t").expect("a valid topic")
}

fn other() -> TopicId {
    TopicId::new("u").expect("a valid topic")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

fn key(name: &str) -> oqueue_core::ObjectKey {
    oqueue_core::ObjectKey::new(name).expect("a valid key")
}

/// One sealed object holding `regions` regions, and its parsed footer.
fn sealed(names: &[(TopicId, i32, u32)]) -> Vec<oqueue_core::Region> {
    let mut builder = BundleBuilder::new();
    for (t, p, count) in names {
        builder
            .push(
                t.clone(),
                partition(*p),
                PushedRecords {
                    count: *count,
                    producer: None,
                },
                &[b'r'; 32],
            )
            .expect("a valid region");
    }
    let bundle = builder.seal().expect("a sealed bundle");
    let payload = bundle.into_payload();
    parse_footer(&payload, payload.len() as u64).expect("a valid footer")
}

#[test]
fn a_manifest_round_trips_every_component_and_region() {
    let first = sealed(&[(topic(), 0, 3), (other(), 1, 5)]);
    let second = sealed(&[(topic(), 0, 7)]);
    let mut builder = CompositeBuilder::new();
    builder.push(key("a"), first.clone()).expect("a component");
    builder.push(key("b"), second.clone()).expect("a component");
    let manifest = builder.seal().expect("a sealed manifest");

    let read = parse_composite(&manifest).expect("a manifest that parses");
    assert_eq!(read.len(), 2);
    assert_eq!(read[0].object(), &key("a"));
    assert_eq!(read[0].regions(), first.as_slice());
    assert_eq!(read[1].object(), &key("b"));
    assert_eq!(read[1].regions(), second.as_slice());
}

/// ⚠️ **One partition may live in several components**, which is the ordinary
/// case: a composite collapses objects that each held a slice of it. Returning
/// the first would serve one slice and lose the rest.
#[test]
fn locating_a_partition_finds_every_run_in_order() {
    let mut builder = CompositeBuilder::new();
    builder
        .push(key("a"), sealed(&[(topic(), 0, 3)]))
        .expect("a component");
    builder
        .push(key("b"), sealed(&[(other(), 0, 9)]))
        .expect("a component");
    builder
        .push(key("c"), sealed(&[(topic(), 0, 4)]))
        .expect("a component");
    let manifest = builder.seal().expect("a sealed manifest");
    let read = parse_composite(&manifest).expect("a manifest that parses");

    let found = locate(&read, &topic(), partition(0));
    assert_eq!(found.len(), 2, "two components hold this partition");
    assert_eq!(found[0].object(), &key("a"));
    assert_eq!(found[0].record_count(), 3);
    assert_eq!(found[1].object(), &key("c"));
    assert_eq!(found[1].record_count(), 4);

    assert!(
        locate(&read, &topic(), partition(7)).is_empty(),
        "a partition no component holds is found nowhere"
    );
}

/// ⚠️ **The failure the magic exists to prevent.** Both formats end in a
/// count, a version and a length, so without it a bundle's trailer parses as a
/// composite's and yields component keys built out of record bytes.
#[test]
fn a_bundle_is_not_a_composite() {
    let mut builder = BundleBuilder::new();
    builder
        .push(
            topic(),
            partition(0),
            PushedRecords {
                count: 2,
                producer: None,
            },
            &[b'r'; 64],
        )
        .expect("a valid region");
    let payload = builder.seal().expect("a sealed bundle").into_payload();
    assert!(
        matches!(parse_composite(&payload), Err(Error::NotACompositeManifest)),
        "a bundle's bytes are refused by name, not parsed into nonsense"
    );
}

#[test]
fn a_component_with_no_region_is_refused() {
    let mut builder = CompositeBuilder::new();
    assert!(matches!(
        builder.push(key("a"), Vec::new()),
        Err(Error::EmptyBundle)
    ));
}

#[test]
fn a_composite_naming_nothing_is_refused() {
    assert!(matches!(
        CompositeBuilder::new().seal(),
        Err(Error::EmptyBundle)
    ));
}

#[test]
fn a_truncated_manifest_is_refused_rather_than_read_short() {
    let mut builder = CompositeBuilder::new();
    builder
        .push(key("a"), sealed(&[(topic(), 0, 3)]))
        .expect("a component");
    let manifest = builder.seal().expect("a sealed manifest");

    for cut in 1..COMPOSITE_TRAILER_LEN.min(manifest.len()) {
        let short = &manifest[..manifest.len() - cut];
        assert!(
            parse_composite(short).is_err(),
            "a manifest missing {cut} trailing byte(s) is refused"
        );
    }
    assert!(parse_composite(&[]).is_err(), "and so are no bytes at all");
}

#[test]
fn a_manifest_whose_declared_length_does_not_fit_is_refused() {
    let mut builder = CompositeBuilder::new();
    builder
        .push(key("a"), sealed(&[(topic(), 0, 3)]))
        .expect("a component");
    let mut manifest = builder.seal().expect("a sealed manifest");
    // The length field sits between the version byte and the magic.
    let len_at = manifest.len() - COMPOSITE_MAGIC.len() - 4;
    manifest[len_at..len_at + 4].copy_from_slice(&9999_u32.to_be_bytes());
    assert!(matches!(
        parse_composite(&manifest),
        Err(Error::MalformedCompositeManifest { .. })
    ));
}

#[test]
fn a_manifest_naming_an_unknown_version_is_refused() {
    let mut builder = CompositeBuilder::new();
    builder
        .push(key("a"), sealed(&[(topic(), 0, 3)]))
        .expect("a component");
    let mut manifest = builder.seal().expect("a sealed manifest");
    let version_at = manifest.len() - COMPOSITE_MAGIC.len() - 4 - 1;
    manifest[version_at] = 99;
    assert!(matches!(
        parse_composite(&manifest),
        Err(Error::UnknownCompositeVersion { version: 99 })
    ));
}

/// ⚠️ **A zero-record region is refused from stored bytes**, not only by the
/// builder: it is durable, indexed, billed and unreachable, and a manifest is
/// a copy of index entries read off the network, so the write-side refusal
/// cannot reach it. `bundle_footer.rs` makes the same call one format over.
#[test]
fn a_region_holding_no_record_is_refused_when_read_back() {
    let mut builder = CompositeBuilder::new();
    builder
        .push(key("a"), sealed(&[(topic(), 0, 3)]))
        .expect("a component");
    let mut manifest = builder.seal().expect("a sealed manifest");

    // The record count sits after the key, the region count, the topic name
    // and the partition: 2 + 1 ("a") + 4 + 2 + 1 ("t") + 4 = 14 bytes in.
    let count_at = 14;
    assert_eq!(
        u32::from_be_bytes([
            manifest[count_at],
            manifest[count_at + 1],
            manifest[count_at + 2],
            manifest[count_at + 3],
        ]),
        3,
        "the fixture's record count is where this test thinks it is"
    );
    manifest[count_at..count_at + 4].copy_from_slice(&0_u32.to_be_bytes());
    assert!(matches!(
        parse_composite(&manifest),
        Err(Error::MalformedCompositeManifest { .. })
    ));
}

/// One object named twice is refused by the builder, so no manifest can carry
/// it.
#[test]
fn a_component_named_twice_is_refused() {
    let mut builder = CompositeBuilder::new();
    builder
        .push(key("a"), sealed(&[(topic(), 0, 3)]))
        .expect("a component");
    assert!(matches!(
        builder.push(key("a"), sealed(&[(topic(), 1, 4)])),
        Err(Error::IndexObjectMismatch)
    ));
}

/// ⚠️ **And a manifest that already names it twice is refused when read**,
/// which the builder's check cannot reach: bytes in a bucket were written by
/// something, and a reader has only the bytes.
#[test]
fn a_manifest_naming_one_object_twice_is_refused_when_read_back() {
    let mut builder = CompositeBuilder::new();
    builder
        .push(key("a"), sealed(&[(topic(), 0, 3)]))
        .expect("a component");
    let manifest = builder.seal().expect("a sealed manifest");

    // Duplicate the body and rewrite the trailer to claim two components.
    let body_len = manifest.len() - COMPOSITE_TRAILER_LEN;
    let mut doubled = Vec::new();
    doubled.extend_from_slice(&manifest[..body_len]);
    doubled.extend_from_slice(&manifest[..body_len]);
    let len = u32::try_from(doubled.len()).expect("a small manifest");
    doubled.extend_from_slice(&2_u32.to_be_bytes());
    doubled.push(1);
    doubled.extend_from_slice(&len.to_be_bytes());
    doubled.extend_from_slice(&COMPOSITE_MAGIC);

    assert!(
        matches!(
            parse_composite(&doubled),
            Err(Error::MalformedCompositeManifest { .. })
        ),
        "two runs over the same bytes are six records where three exist"
    );
}
