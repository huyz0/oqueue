//! The `MaterializedIndex` **read** surface, run against every implementation.
//!
//! ⚠️ **Split from `index.rs` at the 500-line limit, along `ADR-0022`.** That
//! file is the *fold* contract — what an `apply` does, what a refused one does
//! not, what `clear` returns to — and this is the read contract that `ADR-0022`
//! settles separately: which batches a page holds, what bounds it, and what a
//! lookup at the high watermark answers.
//!
//! ⚠️ **The two are different risks and that is the seam.** A fold defect
//! double-counts or loses records; a paging defect answers a fetch with a
//! reference to bytes it should not read — FR-12's zero-GET claim is here, not
//! there, and so is the byte budget the reader spends.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is this test binary's
// only root, and nothing outside it can name these.
#![allow(unreachable_pub)]

use crate::index::{commit, offset, partition, sized_commit, topic};
use oqueue_core::{MAX_BATCHES_PER_PAGE, MaterializedIndex, Offset};

/// FR-12's zero-GET case, at the seam that decides it: a fetch that is
/// already at the high watermark is told to read nothing, so there is
/// nothing for it to GET.
pub fn a_fetch_at_the_high_watermark_finds_no_batches<I: MaterializedIndex>(index: &I) {
    index.apply(&[commit(1, 3), commit(2, 4)]).expect("applied");
    let hwm = index.end_offset(&topic("orders"), partition(0));
    assert_eq!(hwm, offset(7));
    assert_eq!(
        index
            .find_batches(&topic("orders"), partition(0), hwm, u64::MAX)
            .expect("a page"),
        Vec::new(),
        "nothing to read at the end of the log"
    );
    // And past it, which is what a consumer racing a produce asks for.
    assert!(
        index
            .find_batches(&topic("orders"), partition(0), offset(99), u64::MAX)
            .expect("a page")
            .is_empty()
    );
}

/// A partition the index never folded is empty rather than an error — the
/// index knows what the log said, not which topics exist.
pub fn an_unknown_partition_finds_no_batches<I: MaterializedIndex>(index: &I) {
    assert!(
        index
            .find_batches(&topic("ghost"), partition(9), Offset::ZERO, u64::MAX)
            .expect("a page")
            .is_empty()
    );
}

/// Everything from `start` onward, in ascending offset order — not just
/// the one object that contains `start`.
pub fn a_page_runs_from_start_to_the_end_of_the_log<I: MaterializedIndex>(index: &I) {
    // ⚠️ Sized, not `commit`: an unknown length ends the page after one
    // batch, which is the case the test below this one is about.
    for version in 1..=4 {
        index
            .apply(&[sized_commit(version, 2, 10)])
            .expect("applied");
    }
    let page = index
        .find_batches(&topic("orders"), partition(0), offset(3), u64::MAX)
        .expect("a page");
    let bases: Vec<i64> = page
        .iter()
        .map(|b| b.reference().base_offset().get())
        .collect();
    assert_eq!(
        bases,
        vec![2, 4, 6],
        "the object holding offset 3 and every one after it, in order"
    );
}

/// The budget is charged against known lengths, and never returns empty
/// because the first batch is too big.
pub fn the_byte_budget_bounds_the_page_but_always_yields_one<I: MaterializedIndex>(index: &I) {
    index
        .apply(&[
            sized_commit(1, 2, 100),
            sized_commit(2, 2, 100),
            sized_commit(3, 2, 100),
        ])
        .expect("applied");

    let two = index
        .find_batches(&topic("orders"), partition(0), Offset::ZERO, 250)
        .expect("a page");
    assert_eq!(two.len(), 2, "100 + 100 fits in 250, a third does not");

    let one = index
        .find_batches(&topic("orders"), partition(0), Offset::ZERO, 1)
        .expect("a page");
    assert_eq!(
        one.len(),
        1,
        "a budget below the first batch still yields it, or the consumer \
             never advances"
    );
}

/// A batch the index cannot price charges nothing against the byte budget
/// — it is unpriceable, not free, and the page count is what bounds it.
pub fn an_unknown_length_charges_nothing<I: MaterializedIndex>(index: &I) {
    // `commit` uses ByteRange::Full, whose length only the store knows.
    index
        .apply(&[commit(1, 2), commit(2, 2), sized_commit(3, 2, 10)])
        .expect("applied");
    let page = index
        .find_batches(&topic("orders"), partition(0), Offset::ZERO, 10)
        .expect("a page");
    assert_eq!(
        page.len(),
        3,
        "two cannot be priced and pass through the budget; the third is \
         charged against it and exactly fills it"
    );
    assert_eq!(page[0].known_len(), None);
    assert_eq!(page[2].known_len(), Some(10));

    assert_eq!(
        index
            .find_batches(&topic("orders"), partition(0), Offset::ZERO, 5)
            .expect("a page")
            .len(),
        2,
        "the priced one is refused by a budget below it; the unpriceable \
         ones before it are not"
    );
}

/// However many batches a partition has, one page names at most
/// [`MAX_BATCHES_PER_PAGE`] of them — NFR-30's bound on a cold read.
pub fn a_page_is_bounded_by_its_batch_count<I: MaterializedIndex>(index: &I) {
    for version in 1..=(MAX_BATCHES_PER_PAGE as u64 + 5) {
        index.apply(&[commit(version, 1)]).expect("applied");
    }
    assert_eq!(
        index
            .find_batches(&topic("orders"), partition(0), Offset::ZERO, u64::MAX)
            .expect("a page")
            .len(),
        MAX_BATCHES_PER_PAGE
    );
}
