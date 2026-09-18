//! A partition's history as an object: what it round-trips, and what it refuses.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, Error, ManifestEntry, ObjectKey, Offset, PARTITION_MANIFEST_BYTES,
    PARTITION_MANIFEST_MAGIC, PARTITION_MANIFEST_TRAILER_LEN, PartitionManifestBuilder,
    parse_partition_manifest,
};

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a valid key")
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

fn entry(name: &str, base: i64, count: u32) -> ManifestEntry {
    ManifestEntry::new(
        key(name),
        offset(base),
        count,
        ByteRange::bounded(0, u64::from(count) * 32).expect("a valid range"),
    )
    .expect("a valid entry")
}

/// `count` contiguous entries of `per` records each, from offset zero.
fn built(count: usize, per: u32) -> Vec<u8> {
    let mut builder = PartitionManifestBuilder::new();
    for which in 0..count {
        let base = i64::try_from(which).expect("a small count") * i64::from(per);
        builder
            .push(entry(&format!("obj-{which}"), base, per))
            .expect("a contiguous entry");
    }
    builder.seal().expect("a sealed manifest")
}

#[test]
fn a_manifest_round_trips_its_entries_in_offset_order() {
    let manifest = parse_partition_manifest(&built(5, 10)).expect("a manifest that parses");
    assert_eq!(manifest.entries().len(), 5);
    assert_eq!(manifest.entries()[0].object(), &key("obj-0"));
    assert_eq!(manifest.entries()[4].base_offset(), offset(40));
    assert_eq!(
        manifest.entries()[4].end_offset().expect("an end"),
        offset(50)
    );
    assert_eq!(
        manifest.previous(),
        None,
        "the first manifest spills from nothing"
    );
}

/// ⚠️ **The search the whole shape exists for**: a fetch asks which object
/// holds an offset, and gets one answer without reading any object.
#[test]
fn finding_an_offset_returns_the_object_holding_it() {
    let manifest = parse_partition_manifest(&built(5, 10)).expect("a manifest that parses");
    assert_eq!(
        manifest.find(offset(0)).expect("the first object").object(),
        &key("obj-0")
    );
    assert_eq!(
        manifest.find(offset(9)).expect("still the first").object(),
        &key("obj-0")
    );
    assert_eq!(
        manifest.find(offset(10)).expect("the second").object(),
        &key("obj-1")
    );
    assert_eq!(
        manifest.find(offset(49)).expect("the last").object(),
        &key("obj-4")
    );
    assert!(
        manifest.find(offset(50)).is_none(),
        "past the end is the tail's, not this manifest's"
    );
}

/// ⚠️ **Below the first entry is the chain's, not an error.** A manifest that
/// spilled starts above zero, and a reader follows `previous` rather than
/// concluding the records are gone.
#[test]
fn an_offset_below_the_first_entry_is_not_found_here() {
    let mut builder = PartitionManifestBuilder::new().spilling_from(key("older"));
    builder.push(entry("obj-9", 90, 10)).expect("an entry");
    let sealed = builder.seal().expect("a sealed manifest");
    let manifest = parse_partition_manifest(&sealed).expect("a manifest that parses");

    assert_eq!(manifest.previous(), Some(&key("older")));
    assert!(manifest.find(offset(89)).is_none());
    assert_eq!(
        manifest.find(offset(90)).expect("the entry").object(),
        &key("obj-9")
    );
}

#[test]
fn an_entry_that_does_not_meet_the_last_one_is_refused() {
    let mut builder = PartitionManifestBuilder::new();
    builder.push(entry("a", 0, 10)).expect("the first");
    assert!(
        matches!(
            builder.push(entry("b", 11, 10)),
            Err(Error::IndexObjectMismatch)
        ),
        "a gap is records nothing can serve"
    );
    assert!(
        matches!(
            builder.push(entry("c", 9, 10)),
            Err(Error::IndexObjectMismatch)
        ),
        "an overlap is records served twice"
    );
    builder.push(entry("d", 10, 10)).expect("meeting exactly");
}

#[test]
fn an_object_named_twice_is_refused() {
    let mut builder = PartitionManifestBuilder::new();
    builder.push(entry("a", 0, 10)).expect("the first");
    assert!(matches!(
        builder.push(entry("a", 10, 10)),
        Err(Error::IndexObjectMismatch)
    ));
}

#[test]
fn an_entry_holding_no_record_or_an_unbounded_range_is_refused() {
    assert!(matches!(
        ManifestEntry::new(
            key("a"),
            offset(0),
            0,
            ByteRange::bounded(0, 8).expect("a range")
        ),
        Err(Error::EmptyBundle)
    ));
    assert!(matches!(
        ManifestEntry::new(key("a"), offset(0), 4, ByteRange::Full),
        Err(Error::UnboundedRegion)
    ));
}

#[test]
fn a_manifest_naming_nothing_is_refused() {
    assert!(matches!(
        PartitionManifestBuilder::new().seal(),
        Err(Error::EmptyBundle)
    ));
}

/// ⚠️ **What the magic is for.** Every durable format here ends in a count, a
/// version and a length, so without it one parses as another.
#[test]
fn a_composite_is_not_a_partition_manifest() {
    let mut sealed = built(2, 10);
    let magic_at = sealed.len() - PARTITION_MANIFEST_MAGIC.len();
    sealed[magic_at..].copy_from_slice(b"OQCM");
    assert!(matches!(
        parse_partition_manifest(&sealed),
        Err(Error::NotAPartitionManifest)
    ));
}

#[test]
fn a_truncated_manifest_is_refused_rather_than_read_short() {
    let sealed = built(3, 10);
    for cut in 1..PARTITION_MANIFEST_TRAILER_LEN.min(sealed.len()) {
        assert!(
            parse_partition_manifest(&sealed[..sealed.len() - cut]).is_err(),
            "a manifest missing {cut} trailing byte(s) is refused"
        );
    }
    assert!(parse_partition_manifest(&[]).is_err());
}

#[test]
fn a_manifest_naming_an_unknown_version_is_refused() {
    let mut sealed = built(2, 10);
    let version_at = sealed.len() - PARTITION_MANIFEST_MAGIC.len() - 4 - 1;
    sealed[version_at] = 77;
    assert!(matches!(
        parse_partition_manifest(&sealed),
        Err(Error::UnknownPartitionManifestVersion { version: 77 })
    ));
}

/// ⚠️ **A gap written into the bytes is refused on read**, which the builder's
/// own check cannot reach: bytes in a bucket were written by something, and a
/// reader has only the bytes.
#[test]
fn a_gap_in_stored_bytes_is_refused_when_read_back() {
    let mut sealed = built(2, 10);
    // The second entry's base offset: after the 2-byte "no predecessor"
    // length, then entry 0 (2 + 5 key + 8 + 4 + 8 + 8), then entry 1's
    // 2 + 5 key.
    let base_at = 2 + (2 + 5 + 8 + 4 + 8 + 8) + 2 + 5;
    assert_eq!(
        i64::from_be_bytes(
            sealed[base_at..base_at + 8]
                .try_into()
                .expect("eight bytes")
        ),
        10,
        "the fixture's second base offset is where this test thinks it is"
    );
    sealed[base_at..base_at + 8].copy_from_slice(&11_i64.to_be_bytes());
    assert!(matches!(
        parse_partition_manifest(&sealed),
        Err(Error::MalformedPartitionManifest { .. })
    ));
}

/// ⚠️ **The cap bites on the backlog and never on the steady state**, which is
/// `ADR-0042`'s reason for its value: ~4.6 entries after compaction against
/// ~7,200 between rounds.
#[test]
fn the_spill_cap_is_far_above_a_compacted_manifest_and_below_a_round_s_backlog() {
    let compacted = built(5, 524_288).len();
    assert!(
        compacted < PARTITION_MANIFEST_BYTES / 100,
        "a compacted manifest is nowhere near the cap: {compacted} against {PARTITION_MANIFEST_BYTES}"
    );
    let per_entry = (built(101, 10).len() - built(1, 10).len()) / 100;
    assert!(
        per_entry * 7_200 > PARTITION_MANIFEST_BYTES,
        "a round's backlog of 7,200 entries at {per_entry} B each exceeds it, so the chain is reached"
    );
}

/// ⚠️ **A manifest that already names an object twice is refused when read**,
/// which the builder's check cannot reach: bytes in a bucket were written by
/// something, and a reader has only the bytes. Two runs over one object are
/// records served twice, and `find`'s binary search cannot tell them from a
/// healthy manifest.
#[test]
fn an_object_named_twice_in_stored_bytes_is_refused_when_read_back() {
    // Hand-encoded, because the builder cannot emit this: no predecessor,
    // then two entries both keyed `dup`, contiguous so the gap check does not
    // fire first.
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&0_u16.to_be_bytes());
    for base in [0_i64, 10] {
        body.extend_from_slice(&3_u16.to_be_bytes());
        body.extend_from_slice(b"dup");
        body.extend_from_slice(&base.to_be_bytes());
        body.extend_from_slice(&10_u32.to_be_bytes());
        body.extend_from_slice(&0_u64.to_be_bytes());
        body.extend_from_slice(&320_u64.to_be_bytes());
    }
    let body_len = u32::try_from(body.len()).expect("a small manifest");
    body.extend_from_slice(&2_u32.to_be_bytes());
    body.push(1);
    body.extend_from_slice(&body_len.to_be_bytes());
    body.extend_from_slice(&PARTITION_MANIFEST_MAGIC);

    assert!(matches!(
        parse_partition_manifest(&body),
        Err(Error::MalformedPartitionManifest { .. })
    ));
}
