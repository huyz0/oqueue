//! The `MaterializedIndex` **retention** surface, run against every
//! implementation.
//!
//! ⚠️ **Split from `index.rs` at the 500-line limit, along `ADR-0044`**, the
//! way `paging.rs` is split along `ADR-0022`: that file is the fold, and this
//! is what a retention round asks of a partition — its age and where its
//! readable log begins.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect: `main.rs` is the only root of this
// test binary, the call `paging.rs` makes.
#![allow(unreachable_pub)]

use crate::index::{offset, partition, topic};
use oqueue_core::{
    CommitVersion, MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey, Offset, Timestamp,
};

/// ⚠️ **Retention's two questions, answered through the seam**
/// (`ADR-0044`). A round runs inside the coordinator against whichever
/// implementation it was given, so both must answer them — and answer
/// `None` for a partition nothing was committed to, since a round reading
/// the epoch there would reap it on its first sweep.
pub fn retention_reads_a_partition_s_age_and_start<I: MaterializedIndex>(index: &I) {
    let at = |millis: i64| Timestamp::from_millis(millis).expect("a valid time");
    let stamped = |version: u64, when: i64| {
        MetadataEntry::new(
            CommitVersion::new(version),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
                spans: vec![oqueue_core::CommittedSpan::new(
                    topic("orders"),
                    partition(0),
                    10,
                    oqueue_core::ByteRange::Full,
                    None,
                )],
                written_at: at(when),
            },
        )
    };
    assert_eq!(index.time_span(&topic("orders"), partition(0)), None);
    assert_eq!(
        index.log_start(&topic("orders"), partition(0)),
        Offset::ZERO
    );

    index
        .apply(&[stamped(1, 3_000), stamped(2, 7_000)])
        .expect("two stamped commits");
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(3),
            MetadataRecord::Trimmed {
                topic: topic("orders"),
                partition: partition(0),
                start: offset(10),
            },
        )])
        .expect("a trim to the first object's end");

    let span = index
        .time_span(&topic("orders"), partition(0))
        .expect("committed to");
    assert_eq!((span.min(), span.max()), (at(3_000), at(7_000)));
    assert_eq!(index.log_start(&topic("orders"), partition(0)), offset(10));
    assert_eq!(
        index.time_span(&topic("orders"), partition(1)),
        None,
        "a partition nothing was committed to has no age"
    );
}
