//! Reading a footer out of a *suffix*, and what a suffix cannot say.
//!
//! ⚠️ **Split from `bundle_footer.rs` at the 500-line limit, along the
//! question.** That file asks what a parser must refuse about the *bytes*;
//! this asks what it must conclude about the *read*. They are different
//! answers to a failed parse — one means the object is broken, the other means
//! the reader asked for too little of it — and conflating them tells a
//! consumer that live records are permanently gone (`M3.27`).
//!
//! ⚠️ **Why a suffix at all**: a history read resolves a batch through the
//! object's footer, and the object may be far larger than the batch. `M3` GETs
//! the whole object because [`ObjectStore`](oqueue_core::ObjectStore) has no
//! `head`; `M5` reads a tail instead, and these are the cases that road runs
//! into.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is this test binary's
// only root, and nothing outside it can name these.
#![allow(unreachable_pub)]

use crate::bundle_footer::{four_topics, three_regions};
use oqueue_core::{BUNDLE_FORMAT_VERSION, Error, parse_footer};

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

/// ⚠️ **A tail too narrow to hold the footer is not a corrupt object**, and
/// until `M3.27` it was reported as one. A reader that does not know an
/// object's layout GETs its last few kilobytes; when the footer is longer than
/// that, `MalformedBundleFooter` tells it the records are permanently gone,
/// and `retry_class` agrees. What it needs instead is the number the parser
/// just computed: how many trailing bytes would have been enough.
#[test]
fn a_tail_too_narrow_for_the_footer_says_how_many_bytes_it_needed() {
    let whole = three_regions();
    let size = whole.len() as u64;
    let narrow = &whole[whole.len() - (narrowest_tail_that_parses(&whole) - 1)..];

    let Err(Error::BundleTailTooShort { got, needed }) = parse_footer(narrow, size) else {
        panic!("a narrow tail is a narrow read, not a broken object");
    };

    assert_eq!(got, narrow.len() as u64);
    assert!(
        needed > got && needed <= size,
        "and it names a wider read that exists: needed {needed}, got {got}, \
         object {size}"
    );
    // ⚠️ The re-read the number buys, and the point of the whole variant.
    let wider = &whole[whole.len() - usize::try_from(needed).expect("a small object")..];
    parse_footer(wider, size).expect("the object was fine all along");
}

/// ⚠️ **An object genuinely too small to hold what its trailer describes is
/// still corruption**, and the object's size is what separates the two: a
/// wider read cannot exist if the object does not have the bytes.
#[test]
fn an_object_too_small_for_its_own_footer_is_still_malformed() {
    let whole = three_regions();
    let narrow = &whole[whole.len() - (narrowest_tail_that_parses(&whole) - 1)..];

    // The same narrow tail, but the caller says that *is* the whole object.
    assert!(
        matches!(
            parse_footer(narrow, narrow.len() as u64),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "there is no wider read to suggest, so this is a broken object"
    );
}

/// ⚠️ **And the boundary between the two, which is one comparison.** An object
/// exactly as large as the footer it carries is still *readable* — the wider
/// read exists, it is the whole object, and what it finds there (a footer over
/// no payload) is a separate answer this one must not pre-empt. One byte
/// smaller and there is no such read, which is where corruption starts.
#[test]
fn an_object_exactly_as_big_as_its_footer_can_still_be_read_wider() {
    let whole = three_regions();
    let narrow = &whole[whole.len() - (narrowest_tail_that_parses(&whole) - 1)..];
    let Err(Error::BundleTailTooShort { needed, .. }) = parse_footer(narrow, whole.len() as u64)
    else {
        panic!("the fixture's narrow tail is too narrow");
    };

    assert!(
        matches!(
            parse_footer(narrow, needed),
            Err(Error::BundleTailTooShort { .. })
        ),
        "at exactly `needed` the read is still the thing that was too small"
    );
    assert!(
        matches!(
            parse_footer(narrow, needed - 1),
            Err(Error::MalformedBundleFooter { .. })
        ),
        "one byte below it, no read of this object could ever succeed"
    );
}

/// The shortest suffix of `payload` that `parse_footer` accepts.
///
/// ⚠️ Found rather than computed. The trailer's width is the format's business,
/// and a test that hardcoded it would keep passing after the format moved.
fn narrowest_tail_that_parses(payload: &[u8]) -> usize {
    let size = payload.len() as u64;
    (1..=payload.len())
        .find(|n| parse_footer(&payload[payload.len() - n..], size).is_ok())
        .expect("the whole object parses, so some suffix does")
}

/// ⚠️ **A tail too short to hold even the trailer answers with a lower bound**,
/// and that is the one place `needed` is not the whole answer: the field that
/// says how long the footer is has not been read, because it is among the bytes
/// that are missing. A caller that re-reads `needed` once and calls a second
/// failure corruption declares a healthy object broken.
///
/// ⚠️ **Two steps, never more** — the second read holds the trailer, and the
/// trailer holds the real length. That bound is what makes "loop until it
/// succeeds" a safe thing to tell a caller to do.
#[test]
fn a_tail_below_the_trailer_converges_in_one_further_read() {
    let whole = three_regions();
    let size = whole.len() as u64;

    // ⚠️ One byte: below any conceivable trailer, and self-evidently so — a
    // fixture that computed the trailer's width would be asserting the
    // format's business rather than this function's.
    let Err(Error::BundleTailTooShort { needed: first, .. }) =
        parse_footer(&whole[whole.len() - 1..], size)
    else {
        panic!("a tail below the trailer is still a narrow read");
    };

    // The lower bound, read exactly — and it is not yet enough.
    let Err(Error::BundleTailTooShort { needed: second, .. }) = parse_footer(
        &whole[whole.len() - usize::try_from(first).expect("a small object")..],
        size,
    ) else {
        panic!("the first answer is a lower bound, so this one still fails");
    };
    assert!(second > first, "and the second answer is larger: {second}");

    // And the second is the real one.
    parse_footer(
        &whole[whole.len() - usize::try_from(second).expect("a small object")..],
        size,
    )
    .expect("two steps, never more");
}
