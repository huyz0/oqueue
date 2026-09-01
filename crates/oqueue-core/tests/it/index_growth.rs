//! The entry count: the number NFR-11 is about, and the one thing M3 does with
//! it.
//!
//! ⚠️ **Measured, not bounded** (`M3.11`). A ceiling on this index's keying can
//! only be met by evicting, and eviction gives back range a rebuild cannot
//! restore — replaying the log reproduces the same count and sheds the same
//! entries again. `roadmap.md` carries the enforcement to `M5`, beside the
//! coarse per-object keying that makes a bound feasible. What is pinned here
//! is that the number is *right*, because a growth figure nobody can trust is
//! worse than none.

#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, IndexState, MetadataEntry, MetadataRecord, ObjectKey,
    PartitionId, TAIL_WINDOW_ENTRIES, TopicId,
};
use proptest::prelude::*;

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

fn commit(version: u64, records: u32) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic("orders"),
                partition(0),
                records,
                ByteRange::Full,
                None,
            )],
        },
    )
}

/// ⚠️ **The number NFR-11 is about, and the one thing M3 does with it: report
/// it.** It is maintained rather than recomputed, so it has to follow every
/// path — one per span folded, unchanged by demotion between tiers, and back
/// to zero on a clear.
#[test]
fn the_entry_count_follows_every_span_folded() {
    let mut state = IndexState::new();
    assert_eq!(state.entries(), 0);

    state.apply(&[commit(1, 3)]).expect("applied");
    assert_eq!(state.entries(), 1, "one entry per span, not per record");

    // A commit naming several partitions adds one each: the count is of index
    // entries, which is what memory is proportional to.
    state
        .apply(&[MetadataEntry::new(
            CommitVersion::new(2),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new("bundle".to_owned()).expect("a valid key"),
                spans: (0..4)
                    .map(|p| {
                        CommittedSpan::new(topic("orders"), partition(p), 1, ByteRange::Full, None)
                    })
                    .collect(),
            },
        )])
        .expect("applied");
    assert_eq!(state.entries(), 5);

    state.clear();
    assert_eq!(state.entries(), 0);
}

/// ⚠️ **Demotion moves an entry, it does not remove one.** The count is
/// maintained rather than recounted, so a partition whose tail has overflowed
/// into history must still hold every entry it was given.
#[test]
fn the_entry_count_is_unchanged_by_demotion_between_tiers() {
    let mut state = IndexState::new();
    let overflow = TAIL_WINDOW_ENTRIES + 5;
    for version in 1..=overflow {
        state.apply(&[commit(version as u64, 1)]).expect("applied");
    }

    assert_eq!(
        state.history_len(&topic("orders"), partition(0)),
        5,
        "five fell out of the window"
    );
    assert_eq!(
        state.entries(),
        overflow,
        "and every one of them is still an entry"
    );
}

/// ⚠️ **A refused batch adds nothing to the count either.** The fold's
/// guarantee 2 is all-or-nothing, and a maintained counter is exactly the place
/// that quietly stops being true of it: incrementing where the entry is
/// *staged* rather than where it is committed leaves the number counting work
/// that was thrown away.
#[test]
fn a_refused_batch_leaves_the_count_where_it_was() {
    let mut state = IndexState::new();
    state.apply(&[commit(5, 1)]).expect("applied");
    assert_eq!(state.entries(), 1);

    // The second entry repeats a version, so guarantee 2 refuses the batch
    // whole — after the first has already been staged.
    state
        .apply(&[commit(6, 1), commit(6, 1)])
        .expect_err("a repeated version is refused");

    assert_eq!(
        state.entries(),
        1,
        "the staged entry was thrown away, and the count has to have thrown it \
         away too"
    );
}

proptest! {
    /// ⚠️ **The invariant that makes a maintained count worth reading.** It is
    /// added to on every span folded and never recomputed, so nothing but this
    /// says it still equals what the tiers actually hold — and a growth figure
    /// nobody can trust is worse than none.
    #[test]
    fn the_count_equals_what_the_tiers_hold(
        counts in prop::collection::vec((0i32..4, 1u32..8), 1..40),
    ) {
        let mut state = IndexState::new();
        for (version, (part, records)) in counts.iter().enumerate() {
            state
                .apply(&[MetadataEntry::new(
                    CommitVersion::new(version as u64),
                    MetadataRecord::BatchCommitted {
                        object: ObjectKey::new(format!("obj-{version}"))
                            .expect("a valid key"),
                        spans: vec![CommittedSpan::new(
                            topic("orders"),
                            partition(*part),
                            *records,
                            ByteRange::Full,
                            None,
                        )],
                    },
                )])
                .expect("applied");
        }

        let held: usize = (0..4)
            .map(|p| {
                state.tail(&topic("orders"), partition(p)).len()
                    + state.history_len(&topic("orders"), partition(p))
            })
            .sum();
        prop_assert_eq!(state.entries(), held);
    }
}
