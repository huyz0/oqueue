//! What the coordinator's index costs, derived rather than asserted.
//!
//! ⚠️ **`ADR-0043`'s table, held to the types.** Every figure that ADR quotes
//! is growth rate times one of the widths below, and `M5.64` is why they are
//! measured here instead of written down: the manifest-entry width was wrong
//! three times in a row, each time in the same direction, because each
//! correction restated a number rather than deriving one.
//!
//! ⚠️ **Inline is not the cost.** `index_state.rs` has said "~40 bytes per
//! entry" since `M3.6`, which is `size_of::<ObjectRef>()` and omits the object
//! key's own allocation — larger than the struct it hangs off. A budget built
//! on the inline number is short by more than half.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use core::mem::size_of;
use oqueue_core::{BundleNamer, ByteRange, ObjectKey, ObjectRef, Offset, TailEntry, Tiers};

/// A representative object key, at the wide end of `BundleNamer`'s range —
/// see `partition_manifest.rs`'s own `REALISTIC_KEY` for why the wide end is
/// the one a bound quotes.
///
/// ⚠️ **Its width is asserted against `BundleNamer` itself**, not against its
/// own literal. A `assert_eq!(REALISTIC_KEY.len(), 54)` compares a constant to
/// the constant six lines above it and binds nothing — so widening the
/// sequence field or the writer identity would leave every figure in
/// `ADR-0043` stale with no test reding, which is precisely the `M5.64`
/// failure this file's header claims to have ended. Found by `M5.10`'s first
/// round.
const REALISTIC_KEY: &str = "bundles/400000-18d649b1560f6000-1/00000000000000000001";

/// The width `BundleNamer` actually produces for a writer identity of the
/// shape `WriterId::mint` writes — `{pid:x}-{stamp:x}-{nth:x}`, at its wide
/// end: a six-hex pid, a sixteen-hex nanosecond stamp, and a one-hex counter.
fn widest_real_key() -> usize {
    let mut namer = BundleNamer::new("400000-18d649b1560f6000-1").expect("a valid writer");
    namer.next_key().expect("a first key").as_str().len()
}

/// ⚠️ **The three widths `ADR-0043` prices every tier from.** A change to any
/// of these types moves that ADR's table, and reds here first.
#[test]
fn an_index_entry_costs_its_struct_plus_its_object_key() {
    assert_eq!(
        REALISTIC_KEY.len(),
        widest_real_key(),
        "the constant this file prices from is the width BundleNamer writes"
    );
    assert_eq!(REALISTIC_KEY.len(), 54, "and that width is 54 B");

    // `ObjectKey` is a `String`: 24 B inline, and the bytes on the heap.
    assert_eq!(size_of::<ObjectKey>(), 24);
    assert_eq!(size_of::<ObjectRef>(), 40);
    assert_eq!(size_of::<TailEntry>(), 64);

    let history = size_of::<ObjectRef>() + REALISTIC_KEY.len();
    let tail = size_of::<TailEntry>() + REALISTIC_KEY.len();
    let manifest = size_of::<ObjectKey>() + size_of::<Offset>() + REALISTIC_KEY.len();
    assert_eq!(history, 94, "a history entry");
    assert_eq!(tail, 118, "a tail entry");
    assert_eq!(manifest, 86, "a manifest reference");

    // ⚠️ **And the function the quota reads prices each tier at the same
    // width** (`M5.71`). `Tiers::bytes` recomputes these from `size_of`
    // rather than importing them, so without this the two derivations could
    // drift apart silently — which is `M5.64`'s failure with one more file in
    // it. One entry of each tier, so a wrong width shows as itself.
    assert_eq!(
        Tiers {
            tail: 1,
            history: 0,
            manifests: 0
        }
        .bytes(REALISTIC_KEY.len()),
        tail
    );
    assert_eq!(
        Tiers {
            tail: 0,
            history: 1,
            manifests: 0
        }
        .bytes(REALISTIC_KEY.len()),
        history
    );
    assert_eq!(
        Tiers {
            tail: 0,
            history: 0,
            manifests: 1
        }
        .bytes(REALISTIC_KEY.len()),
        manifest
    );
}

/// ⚠️ **The finding `M5.10` exists to record**: at doc 14 §3's working set and
/// `ADR-0042`'s thirty-minute sweep, un-absorbed history is **44.8 times** the
/// tail.
///
/// ⚠️ **Stated to one decimal, and asserted in tenths.** "Forty-four" is
/// `676.8 ÷ 15.104` truncated, and this file's own convention rounds — which
/// is how a *true* forty-five became a false forty-four in the commit written
/// to stop a correction from restating a truncation. `M5.74`'s third round. ⚠️ **That is a statement about 1800 seconds, not about the
/// tiers** — they cross at forty seconds, which the test below pins. The row's
/// premise is right that the tail is real and wrong that it is therefore the
/// thing to build a mechanism for.
#[test]
fn un_absorbed_history_dwarfs_the_tail_at_the_working_set() {
    // Doc 14 §3: 1M active partitions, 4M (object, partition) pairs per second.
    let partitions = 1_000_000_usize;
    let spans_per_second = 4_000_000_usize;
    let tail_window = oqueue_core::TAIL_WINDOW_ENTRIES;
    let history = size_of::<ObjectRef>() + REALISTIC_KEY.len();
    let tail = size_of::<TailEntry>() + REALISTIC_KEY.len();

    let tail_bytes = partitions * tail_window * tail;
    assert_eq!(tail_bytes, 15_104_000_000, "the tail is 15.1 GB");

    // `ADR-0042`'s own sweep interval: 30 minutes between compaction rounds.
    let sweep_seconds = 1_800_usize;
    let history_bytes = spans_per_second * sweep_seconds * history;
    assert_eq!(
        history_bytes, 676_800_000_000,
        "un-absorbed history is 676.8 GB"
    );
    assert_eq!(
        history_bytes * 10 / tail_bytes,
        448,
        "44.8 times the tier the row called the problem"
    );
}

/// ⚠️ **The interval a memory budget buys**, which is what `M5.16` now has to
/// choose against. History grows at the span rate times a history entry, so
/// the interval is a division — and stating it as one keeps the table in
/// `ADR-0043` derived rather than copied.
#[test]
fn a_memory_budget_names_a_sweep_interval() {
    let per_second = 4_000_000_usize * (size_of::<ObjectRef>() + REALISTIC_KEY.len());
    assert_eq!(per_second / 1_000_000, 376, "history grows at 376 MB/s");

    // ⚠️ **And doc 14 §3's other row, at the measured width.** Its table
    // prices these at 160 MB/s and 16 KB/s, which are 40 B per entry — the
    // inline width. The *ratio* between the rows is what that table is about
    // and is unchanged; neither absolute figure is one to size anything from.
    let per_object = 400_usize * (size_of::<ObjectRef>() + REALISTIC_KEY.len());
    assert_eq!(
        per_object, 37_600,
        "per-object entries grow it at 37.6 KB/s"
    );
    assert_eq!(
        per_second / per_object,
        10_000,
        "ten thousand times, which is the ratio doc 14 §3's table is about"
    );

    // ⚠️ **Milliseconds, because every coarser unit truncates.** Tenths were
    // tried and 2.6596 s truncated to "2.6" while the table rounds it to 2.7 —
    // the same defect at a finer grain, found in the round that fixed it at
    // the last one. A millisecond is three digits below what the prose prints,
    // so the rounding it does is unambiguous and the assertion is exact.
    for (budget_gb, millis) in [(1_usize, 2_659_usize), (8, 21_276), (32, 85_106)] {
        let budget = budget_gb * 1_000_000_000;
        assert_eq!(
            budget * 1_000 / per_second,
            millis,
            "a {budget_gb} GB budget buys {millis} ms between sweeps"
        );
    }
}

/// A `ByteRange` is what a tail entry carries and a history entry does not,
/// and the difference is the whole of the two tiers' cost gap — 24 B, which is
/// why demoting a tail entry to history saves a fifth rather than a tier.
#[test]
fn the_tail_pays_for_its_byte_range_and_nothing_else() {
    assert_eq!(
        size_of::<TailEntry>() - size_of::<ObjectRef>(),
        size_of::<ByteRange>(),
        "a tail entry is a history entry plus its range"
    );
    assert_eq!(size_of::<ByteRange>(), 24);
}

/// ⚠️ **Which tier dominates depends on the sweep interval, and they cross at
/// forty seconds.** The tail is a steady state and un-absorbed history is an
/// accumulation, so "history is 44.8 times the tail" is a statement
/// about a *thirty-minute* sweep — the interval `ADR-0043`'s own decision
/// abolishes. Two of the three intervals it tabulates sit below the crossing
/// and the largest does not, so which tier is larger is a property of the
/// interval chosen — and a quota that bounded history alone would let a node
/// at the 21 s row hold 23 GB without firing. `M5.10`'s first round measured
/// the crossing; its third found this sentence still asserting the
/// overcorrection three commits after the ADR had dropped it.
#[test]
fn the_two_tiers_cross_at_forty_seconds() {
    let partitions = 1_000_000_usize;
    let tail_bytes = partitions
        * oqueue_core::TAIL_WINDOW_ENTRIES
        * (size_of::<TailEntry>() + REALISTIC_KEY.len());
    let per_second = 4_000_000_usize * (size_of::<ObjectRef>() + REALISTIC_KEY.len());

    assert_eq!(
        tail_bytes * 10 / per_second,
        401,
        "they are equal at 40.2 s"
    );

    // At the 8 GB history budget `ADR-0043` tabulates, the tail is the larger
    // term; at its 32 GB row it is the smaller. That is why the quota's
    // subject is the total and not one tier.
    let history_at_21s = per_second * 21;
    assert!(
        tail_bytes > history_at_21s,
        "at a 21 s sweep the tail is the larger term: {tail_bytes} against {history_at_21s}"
    );
    assert_eq!(
        tail_bytes / history_at_21s,
        1,
        "though within a factor of two"
    );

    // ⚠️ **And at the largest interval the ADR tabulates, history is larger
    // again.** Pinning only the lower side of the crossing is how "at the
    // intervals it recommends the tail is the larger term" survived three
    // rounds — it was true of two rows of a three-row table and
    // asserted nowhere. `M5.74`'s second round.
    let history_at_85s = per_second * 85;
    assert!(
        history_at_85s > tail_bytes,
        "at an 85 s sweep history is the larger term: {history_at_85s} against {tail_bytes}"
    );

    // ⚠️ **The sum, which is the number the quota's subject turns on.** Both
    // addends were computed here and never added, so `ADR-0043`'s "a node
    // would hold 23 GB without the quota firing" was the one figure in it that
    // no assertion produced.
    assert_eq!(
        tail_bytes + history_at_21s,
        23_000_000_000,
        "a 21 s sweep leaves a node holding 23 GB across the two tiers"
    );
}

/// ⚠️ **The figures `ADR-0043` corrects in `ADR-0042`'s table, derived.** They
/// were quoted there and in `M5.73`'s row while the row's own closing sentence
/// claimed every figure was derived by this file — the fourth minor of
/// `M5.10`'s third round, and the same propagation failure the ADR is about.
#[test]
fn the_state_table_adr_0042_priced_at_inline_widths() {
    let history = size_of::<ObjectRef>() + REALISTIC_KEY.len();
    let tail = size_of::<TailEntry>() + REALISTIC_KEY.len();
    let manifest = size_of::<ObjectKey>() + size_of::<Offset>() + REALISTIC_KEY.len();

    // `ADR-0042` Quantity 1's own entry counts, at this file's widths.
    let per_object_partition = 2_400_000_000_000_usize; // 4M/s x 7 days
    let per_object = 240_000_000_usize; // 400/s x 7 days
    let partitions = 1_000_000_usize;

    // ⚠️ **Exact bytes, and the prose rounds them.** Two other conventions
    // were tried and both misled: a whole-unit assertion truncates, so the
    // per-object row's `== 22` accepted anything up to 23 GB and would have
    // left the ADR half a gigabyte stale with the test green; and asserting in
    // tenths *also* truncates, which is how 22.56 GB came to be printed as
    // "22.5" one finding after 225.6 TB had been printed as "225". A byte
    // count has one value, the prose rounds it, and the two cannot disagree.
    assert_eq!(
        partitions * oqueue_core::TAIL_WINDOW_ENTRIES * tail,
        15_104_000_000,
        "the tail, 15.1 GB, where ADR-0042 priced 7.2 GB"
    );
    assert_eq!(
        per_object_partition * history,
        225_600_000_000_000,
        "per (object, partition), 225.6 TB, where ADR-0042 priced 97 TB"
    );
    assert_eq!(
        per_object * history,
        22_560_000_000,
        "per object, 22.6 GB to one decimal, where ADR-0042 priced 9.7 GB"
    );
    assert_eq!(
        partitions * manifest,
        86_000_000,
        "manifest references, 86 MB, where ADR-0042 priced 40 MB"
    );
}

/// What demoting a tail entry to history saves, which `ADR-0043` decision 4
/// prices at 3.1 GB before declining to choose between the levers.
#[test]
fn demoting_the_tail_saves_its_byte_range_and_no_more() {
    let saved = size_of::<ByteRange>();
    let entries = 1_000_000_usize * oqueue_core::TAIL_WINDOW_ENTRIES;
    assert_eq!(saved, 24, "a tail entry's range is what demotion drops");
    assert_eq!(
        entries * saved,
        3_072_000_000,
        "3.1 GB to one decimal, which is three times the 1 GB history budget"
    );
    let tail = entries * (size_of::<TailEntry>() + REALISTIC_KEY.len());
    assert_eq!(tail, 15_104_000_000, "of the tail's 15.1 GB");
}
