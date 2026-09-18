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
use oqueue_core::{BundleNamer, ByteRange, ObjectKey, ObjectRef, Offset, TailEntry};

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
}

/// ⚠️ **The finding `M5.10` exists to record**: at doc 14 §3's working set the
/// tail is the *smaller* term by forty-five times, and the row's own premise —
/// that the tail is what no keying fixes — is what this refutes.
#[test]
fn un_absorbed_history_dwarfs_the_tail_at_the_working_set() {
    // Doc 14 §3: 1M active partitions, 4M (object, partition) pairs per second.
    let partitions = 1_000_000_usize;
    let spans_per_second = 4_000_000_usize;
    let tail_window = oqueue_core::TAIL_WINDOW_ENTRIES;
    let history = size_of::<ObjectRef>() + REALISTIC_KEY.len();
    let tail = size_of::<TailEntry>() + REALISTIC_KEY.len();

    let tail_bytes = partitions * tail_window * tail;
    assert_eq!(tail_bytes / 1_000_000_000, 15, "the tail is 15 GB");

    // `ADR-0042`'s own sweep interval: 30 minutes between compaction rounds.
    let sweep_seconds = 1_800_usize;
    let history_bytes = spans_per_second * sweep_seconds * history;
    assert_eq!(
        history_bytes / 1_000_000_000,
        676,
        "un-absorbed history is 677 GB (676 by integer division)"
    );
    assert_eq!(
        history_bytes / tail_bytes,
        44,
        "which is forty-four times the tier the row called the problem"
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

    for (budget_gb, seconds) in [(1_usize, 2_usize), (8, 21), (32, 85)] {
        let budget = budget_gb * 1_000_000_000;
        assert_eq!(
            budget / per_second,
            seconds,
            "a {budget_gb} GB budget buys {seconds} s between sweeps"
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
/// accumulation, so "history is forty-four times the tail" is a statement
/// about a *thirty-minute* sweep — the interval `ADR-0043`'s own decision
/// abolishes. At the intervals it recommends the tail is the larger term, and
/// a quota that bounded history alone would let a node hold 23 GB without
/// firing. `M5.10`'s first round measured it.
#[test]
fn the_two_tiers_cross_at_forty_seconds() {
    let partitions = 1_000_000_usize;
    let tail_bytes = partitions
        * oqueue_core::TAIL_WINDOW_ENTRIES
        * (size_of::<TailEntry>() + REALISTIC_KEY.len());
    let per_second = 4_000_000_usize * (size_of::<ObjectRef>() + REALISTIC_KEY.len());

    assert_eq!(tail_bytes / per_second, 40, "they are equal at 40 s");

    // At the 8 GB history budget `ADR-0043` tabulates, the tail is the larger
    // term — which is why the quota's subject is the total and not one tier.
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
}
