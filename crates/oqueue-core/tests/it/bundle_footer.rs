//! Reading a footer back, including from bytes nobody sane wrote.
//!
//! ⚠️ **Split from `bundle.rs`'s suite at the 500-line limit, along the risk.**
//! That file is about what a flush *builds*; this is about what a reader does
//! with an object store's answer — where the inputs are not this process's, so
//! `security.md` rule 3 (no panic reachable from stored bytes) and the
//! disjointness the builder guarantees on write and the parser must
//! re-establish on read are what is at stake.

#![allow(clippy::expect_used)]

use oqueue_core::{
    BUNDLE_FORMAT_VERSION, BundleBuilder, ByteRange, Error, PartitionId, RegionAlg, TopicId,
    parse_footer,
};

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

/// The footer round-trips: what a reader parses back is what the writer meant.
#[test]
fn the_footer_reads_back_exactly_what_was_written() {
    let sealed = four_topics().seal().expect("regions were pushed");
    let read =
        parse_footer(sealed.payload(), sealed.payload().len() as u64).expect("the footer parses");
    assert_eq!(read, sealed.regions(), "field for field, in order");
    assert!(
        read.iter().all(|region| region.alg() == RegionAlg::None),
        "doc 10 #40's field exists from this commit, and M3 writes `none`"
    );
}

/// ⚠️ **A reader has the object's size, not the object.** It GETs the tail, so
/// parsing must work from any suffix that contains the whole footer.
#[test]
fn the_footer_parses_from_the_object_s_tail_alone() {
    let sealed = four_topics().seal().expect("regions were pushed");
    let whole =
        parse_footer(sealed.payload(), sealed.payload().len() as u64).expect("the footer parses");

    let payload = sealed.payload();
    // The footer's own declared length is what a reader sizes its tail GET
    // from; anything at least that long reads the same footer.
    let trailer = &payload[payload.len() - 4..];
    let footer_len =
        u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]) as usize + 9;
    for tail in [footer_len, footer_len + 17, payload.len()] {
        let from = payload.len() - tail;
        assert_eq!(
            parse_footer(&payload[from..], payload.len() as u64)
                .expect("a tail containing the footer"),
            whole,
            "a {tail}-byte tail reads the same footer"
        );
    }

    // ⚠️ And a tail too short to hold the footer is refused rather than
    // half-parsed — a reader that guessed too small must be told, not handed
    // some of the regions.
    assert!(
        parse_footer(
            &payload[payload.len() - (footer_len - 1)..],
            payload.len() as u64
        )
        .is_err(),
        "one byte short of the footer is not a footer"
    );
}

/// ⚠️ **No truncation panics**, which is the claim and the whole of it —
/// `security.md` rule 3, on bytes an object store returned. Some truncations
/// legitimately *parse*, because a shorter object could have carried the footer
/// they end at, so "refused" would be the wrong assertion and a name promising
/// it would be worse than none.
///
/// ⚠️ **This is not adversarial coverage.** It walks one payload's suffixes and
/// crafts no `count`, `footer_len`, `name_len` or range — which is where a
/// decoder over stored bytes actually breaks. `security.md` rule 5 and
/// `testing.md` rule 24 ask for a fuzz target, and `M3.18`'s row now names it
/// — ⚠️ **including teaching `scripts/fuzz.sh` to look here**, which it cannot
/// today: it walks `crates/oqueue-codec/src/*.rs` only, so a decoder in
/// `oqueue-core` is not merely untargeted, it is invisible to the gate that
/// would say so.
#[test]
fn no_truncation_of_the_tail_makes_the_parser_panic() {
    let sealed = four_topics().seal().expect("regions were pushed");
    let payload = sealed.payload();

    for cut in 1..payload.len().min(200) {
        let truncated = &payload[..payload.len() - cut];
        // Either it fails, or it parses a footer that a shorter object could
        // legitimately have carried; what it must never do is panic.
        let _ = parse_footer(truncated, truncated.len() as u64);
    }

    assert!(
        parse_footer(&[], 0).is_err(),
        "nothing at all is not a footer"
    );
    assert!(
        parse_footer(&[0, 0, 0, 0, BUNDLE_FORMAT_VERSION, 0, 0, 0, 0], 9).is_err(),
        "a trailer declaring no regions is refused"
    );
}

/// ⚠️ **The invariant the *builder* holds, re-established on read.** A footer
/// is bytes an object store returned, so a corrupt length — or a bug in
/// whatever rewrites these objects later — yields regions no builder would
/// produce. A region reaching past the payload, or overlapping its neighbour,
/// would have a reader serve one topic's consumer another topic's records.
#[test]
fn a_footer_whose_regions_overlap_or_overrun_is_refused() {
    let sealed = four_topics().seal().expect("regions were pushed");
    let payload = sealed.payload();
    let trailer = &payload[payload.len() - 4..];
    let footer_len = usize::try_from(u32::from_be_bytes([
        trailer[0], trailer[1], trailer[2], trailer[3],
    ]))
    .expect("a small footer");
    let footer_at = payload.len() - 9 - footer_len;

    // The first region's length is the eight bytes before its algorithm byte.
    let first_name_len = usize::from(u16::from_be_bytes([
        payload[footer_at],
        payload[footer_at + 1],
    ]));
    let length_at = footer_at + 2 + first_name_len + 4 + 4 + 8;

    let mut overrun = payload.to_vec();
    overrun[length_at..length_at + 8].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(
        matches!(
            parse_footer(&overrun, overrun.len() as u64),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "a region claiming the whole object is refused, not handed to a reader"
    );

    let mut overlap = payload.to_vec();
    // Long enough to reach into the second region, short enough to fit the
    // payload — so only the disjointness check can catch it.
    overlap[length_at..length_at + 8].copy_from_slice(&40_u64.to_be_bytes());
    assert!(
        matches!(
            parse_footer(&overlap, overlap.len() as u64),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "and so is one that reaches into its neighbour"
    );
}

/// ⚠️ **An empty region is refused on the way in *and* on the way out.**
/// `BundleBuilder::push` rejects no bytes or no records because such a region
/// is durable, indexed, billed and unreachable with no error anywhere; a footer
/// that arrives claiming it has the same effect, and the builder's refusal
/// cannot reach bytes this process did not write.
///
/// ⚠️ **The two halves are refused in different places**, which is worth
/// pinning rather than assuming: a zero record count is caught by the parser's
/// own range check, while a zero-length range never becomes a `Region` at all —
/// `ByteRange::bounded` refuses it as `EmptyByteRange`.
#[test]
fn a_region_carrying_no_records_or_no_bytes_is_refused_on_the_read_path_too() {
    let sealed = four_topics().seal().expect("regions were pushed");
    let payload = sealed.payload();
    let trailer = &payload[payload.len() - 4..];
    let footer_len = usize::try_from(u32::from_be_bytes([
        trailer[0], trailer[1], trailer[2], trailer[3],
    ]))
    .expect("a small footer");
    let footer_at = payload.len() - 9 - footer_len;
    let first_name_len = usize::from(u16::from_be_bytes([
        payload[footer_at],
        payload[footer_at + 1],
    ]));
    // name_len, name, partition — then the record count, then offset, length.
    let count_at = footer_at + 2 + first_name_len + 4;
    let length_at = count_at + 4 + 8;

    let mut no_records = payload.to_vec();
    no_records[count_at..count_at + 4].copy_from_slice(&0_u32.to_be_bytes());
    assert!(
        matches!(
            parse_footer(&no_records, no_records.len() as u64),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "bytes no offset will ever be assigned to are refused, not indexed"
    );

    let mut no_bytes = payload.to_vec();
    no_bytes[length_at..length_at + 8].copy_from_slice(&0_u64.to_be_bytes());
    assert!(
        matches!(
            parse_footer(&no_bytes, no_bytes.len() as u64),
            Err(Error::EmptyByteRange)
        ),
        "and so is a region promising records in no bytes"
    );
}

/// ⚠️ **The payload's edge, exactly.** A region ending on the last payload byte
/// is fine; one byte further reaches into the footer. The boundary is computed
/// from the object's size, which is why `parse_footer` takes it — a tail alone
/// cannot say where the payload ends.
#[test]
fn a_region_may_end_on_the_last_payload_byte_and_not_one_past_it() {
    let sealed = four_topics().seal().expect("regions were pushed");
    let payload = sealed.payload();
    let trailer = &payload[payload.len() - 4..];
    let footer_len = usize::try_from(u32::from_be_bytes([
        trailer[0], trailer[1], trailer[2], trailer[3],
    ]))
    .expect("a small footer");
    let payload_end = payload.len() - 9 - footer_len;

    // The last region's length field: rewrite it to end exactly at the edge,
    // then one byte past.
    let last = sealed.regions().last().expect("four regions");
    let ByteRange::Bounded(bounds) = last.bytes() else {
        panic!("bounded");
    };
    let start = usize::try_from(bounds.offset()).expect("a small payload");
    let length_at = payload.len() - 9 - 9;

    let mut exact = payload.to_vec();
    let fits = u64::try_from(payload_end - start).expect("a small payload");
    exact[length_at..length_at + 8].copy_from_slice(&fits.to_be_bytes());
    assert!(
        parse_footer(&exact, exact.len() as u64).is_ok(),
        "a region ending on the last payload byte is fine"
    );

    let mut past = payload.to_vec();
    past[length_at..length_at + 8].copy_from_slice(&(fits + 1).to_be_bytes());
    assert!(
        matches!(
            parse_footer(&past, past.len() as u64),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "and one byte further reaches into the footer"
    );
}

/// A version this build does not know is refused rather than guessed at.
#[test]
fn an_unknown_format_version_is_refused() {
    let sealed = four_topics().seal().expect("regions were pushed");
    let mut payload = sealed.into_payload();
    let version_at = payload.len() - 5;
    payload[version_at] = BUNDLE_FORMAT_VERSION.wrapping_add(1);

    assert_eq!(
        parse_footer(&payload, payload.len() as u64),
        Err(Error::UnknownBundleFormat {
            version: BUNDLE_FORMAT_VERSION.wrapping_add(1)
        })
    );
}

/// ⚠️ An unknown region algorithm is an **error, never a default**. Reading it
/// as "stored as written" would hand a decoder ciphertext.
#[test]
fn an_unknown_region_algorithm_is_refused_rather_than_defaulted() {
    let mut bundle = BundleBuilder::new();
    bundle
        .push(topic("orders"), partition(0), 1, b"records")
        .expect("a non-empty region");
    let sealed = bundle.seal().expect("a region was pushed");
    let mut payload = sealed.into_payload();

    // The single region's algorithm byte is the last of the footer, which the
    // nine-byte trailer follows.
    let alg_at = payload.len() - 10;
    assert_eq!(payload[alg_at], RegionAlg::None.code());
    payload[alg_at] = 7;

    assert_eq!(
        parse_footer(&payload, payload.len() as u64),
        Err(Error::UnknownRegionAlg { code: 7 })
    );
}
