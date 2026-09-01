//! Reading a footer back, including from bytes nobody sane wrote.
//!
//! ⚠️ **Split from `bundle.rs`'s suite at the 500-line limit, along the risk.**
//! That file is about what a flush *builds*; this is about what a reader does
//! with an object store's answer — where the inputs are not this process's, so
//! `security.md` rule 3 (no panic reachable from stored bytes) and the
//! disjointness the builder guarantees on write and the parser must
//! re-establish on read are what is at stake.
//!
//! ⚠️ **The sharp cases are the ones that still *parse*** (`M3.27`). A tail
//! that is obvious rubbish is caught by any check; a region whose length field
//! is one byte short leaves a footer that is well-formed in every respect
//! except that it no longer describes its own payload, and a reader honouring
//! it serves a truncated batch as a topic's records. Nothing downstream can
//! tell, because the bytes it gets are valid — they are simply not all of
//! them.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is this test binary's
// only root, so nothing outside it can name these. The workspace's
// `unreachable_pub = "deny"` is right about library crates and has no way to
// tell a test binary's shared module apart from one.
#![allow(unreachable_pub)]

use oqueue_core::{
    BUNDLE_FORMAT_VERSION, BundleBuilder, ByteRange, Error, PartitionId, PushedRecords, RegionAlg,
    TopicId, parse_footer,
};

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

/// A flush spanning four topics.
pub fn four_topics() -> BundleBuilder {
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
                PushedRecords {
                    count: n + 1,
                    producer: None,
                },
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
        .push(
            topic("orders"),
            partition(0),
            PushedRecords {
                count: 1,
                producer: None,
            },
            b"records",
        )
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

/// ⚠️ **A gap in the middle of a payload is what shortening a length field
/// produces**, and it is the sharp case: the footer still parses, every region
/// is still ascending and disjoint, and the reader hands that topic's consumer
/// a *truncated batch* as its records. Nothing downstream can tell — the bytes
/// are well-formed, they are simply not all of them — so this is silent
/// wrongness reachable from stored bytes, which is what `check_regions` exists
/// to make impossible.
#[test]
fn a_region_whose_length_was_shortened_no_longer_parses() {
    let good = three_regions();
    let size = good.len() as u64;
    parse_footer(&good, size).expect("the object as written parses");

    // The first region's `length`, one byte shorter: a gap of one byte opens
    // between region 0's end and region 1's offset.
    let shortened = with_first_length(&good, |length| length - 1);

    assert!(
        matches!(
            parse_footer(&shortened, size),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "a one-byte gap is a region that no longer describes its own bytes"
    );
}

/// ⚠️ **And the *last* region's length, which the pairwise check cannot see.**
/// There is no next region to disagree with it, so shortening it leaves a gap
/// between the payload's last byte and the footer — the same silent truncation,
/// at the one position an ascending-and-disjoint rule is blind to.
#[test]
fn the_last_regions_length_is_checked_against_the_payloads_end() {
    let good = three_regions();
    let size = good.len() as u64;

    let shortened = with_last_length(&good, |length| length - 1);

    assert!(
        matches!(
            parse_footer(&shortened, size),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "the final region has to end where the payload does"
    );
}

/// Three regions in one payload, as the builder writes them.
///
/// ⚠️ **The record lengths are chosen, not arbitrary.** Each region's `length`
/// field is located below by searching for its value, and a contiguous layout
/// makes region *n*'s length equal region *n+1*'s offset by construction — so
/// the helpers assert how many times each value may appear, and lengths that
/// collided with anything else would make that assertion unprovable.
pub fn three_regions() -> Vec<u8> {
    let mut bundle = BundleBuilder::new();
    for (name, records) in [
        ("orders", &b"aaaaa"[..]),
        ("payments", &b"bbbbbbb"[..]),
        ("shipments", &b"ccccccccccc"[..]),
    ] {
        bundle
            .push(
                topic(name),
                partition(0),
                PushedRecords {
                    count: 1,
                    producer: None,
                },
                records,
            )
            .expect("a non-empty region");
    }
    bundle.seal().expect("regions were pushed").into_payload()
}

/// The payload with the first region's `length` field rewritten.
///
/// ⚠️ **Its value appears exactly twice** — as this region's length and as the
/// next region's offset, which contiguity makes the same number — and the
/// first of the two is the length, because a region's own offset is written
/// before its length and region zero's offset is `0`.
fn with_first_length(payload: &[u8], change: impl Fn(u64) -> u64) -> Vec<u8> {
    rewrite_length(payload, 0, 2, change)
}

/// The payload with the last region's `length` field rewritten.
///
/// ⚠️ **Its value appears exactly once**: there is no region after it for
/// contiguity to make an offset out of it.
fn with_last_length(payload: &[u8], change: impl Fn(u64) -> u64) -> Vec<u8> {
    let count = parse_footer(payload, payload.len() as u64)
        .expect("the object as written parses")
        .len();
    rewrite_length(payload, count - 1, 1, change)
}

/// The payload with one region's `length` field rewritten in place.
///
/// ⚠️ **Located by value, and the count is asserted** — which is the only thing
/// that makes locating by value sound. An earlier version of this helper took
/// the *last* match, silently rewrote the following region's **offset**
/// instead, and produced an overlap rather than a gap: the test passed against
/// the check it was written to fail, because a different check caught a
/// different defect. `testing.md` rule 5 — a test that cannot fail is not
/// evidence.
fn rewrite_length(
    payload: &[u8],
    which: usize,
    expected: usize,
    change: impl Fn(u64) -> u64,
) -> Vec<u8> {
    let regions = parse_footer(payload, payload.len() as u64).expect("it parses");
    let ByteRange::Bounded(bounds) = regions[which].bytes() else {
        panic!("a region is always bounded");
    };
    let old = bounds.length().to_be_bytes();
    let new = change(bounds.length()).to_be_bytes();
    let at: Vec<usize> = payload
        .windows(8)
        .enumerate()
        .filter(|(_, window)| *window == old)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        at.len(),
        expected,
        "the length value must appear exactly where this helper expects it"
    );
    // ⚠️ **The first match, and `expected` is what makes that sound.** A
    // region's own offset is written before its length, and region zero's
    // offset is `0` — so when a value appears twice, the earlier of the two is
    // this region's length and the later is the next region's offset. An
    // `Occurrence` parameter stood here and selected nothing, which read as if
    // the choice had been made when it had not.
    let at = at[0];
    let mut out = payload.to_vec();
    out[at..at + 8].copy_from_slice(&new);
    out
}
