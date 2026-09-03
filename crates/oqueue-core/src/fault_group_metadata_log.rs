//! A [`GroupMetadataLog`] decorator that can be made to refuse — `M4.14`.
//!
//! ⚠️ **A decorator over any [`GroupMetadataLog`], not a second fake** —
//! `FaultMetadataLog`'s own precedent, unchanged reasoning: what it adds is
//! refusal; what it stores when it is not refusing is whatever the log
//! underneath stores.
//!
//! ⚠️ **Its own small decorator, not `FaultMetadataLog` made generic** —
//! `ADR-0035`'s own "two traits, no shared code" consequence, accepted
//! there rather than solved with an early, unproven generalization over two
//! data points.

use crate::{BoxFuture, CommitVersion, Error, GroupMetadataEntry, GroupMetadataLog, Result};
use std::sync::atomic::{AtomicBool, Ordering};

/// A [`GroupMetadataLog`] that can be told to refuse every `append`.
#[derive(Debug)]
pub struct FaultGroupMetadataLog<L> {
    inner: L,
    refuse_append: AtomicBool,
}

impl<L> FaultGroupMetadataLog<L> {
    /// Wraps `inner`, injecting nothing until told to.
    pub const fn new(inner: L) -> Self {
        Self {
            inner,
            refuse_append: AtomicBool::new(false),
        }
    }

    /// Every following `append` resolves [`Error::Transient`] without
    /// storing anything, until [`Self::heal`] is called.
    pub fn refuse_append(&self) {
        self.refuse_append.store(true, Ordering::SeqCst);
    }

    /// Stops refusing.
    pub fn heal(&self) {
        self.refuse_append.store(false, Ordering::SeqCst);
    }
}

impl<L: GroupMetadataLog> GroupMetadataLog for FaultGroupMetadataLog<L> {
    fn append<'a>(&'a self, entries: &'a [GroupMetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.refuse_append.load(Ordering::SeqCst) {
                return Err(Error::Transient);
            }
            self.inner.append(entries).await
        })
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<GroupMetadataEntry>>> {
        self.inner.read_from(start, max_entries)
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        self.inner.last_version()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::FaultGroupMetadataLog;
    use crate::test_executor::block_on;
    use crate::{
        CommitVersion, Error, FakeGroupMetadataLog, GroupId, GroupMetadataEntry, GroupMetadataLog,
        GroupMetadataRecord, TopicId,
    };

    fn entry_at(version: u64, offset: i64) -> GroupMetadataEntry {
        GroupMetadataEntry::new(
            CommitVersion::new(version),
            GroupMetadataRecord::OffsetCommitted {
                group: GroupId::new("g").expect("non-empty"),
                topic: TopicId::new("t").expect("non-empty"),
                partition: 0,
                offset,
            },
        )
    }

    /// Three states in one test, in order, so `refuse_append` and `heal`
    /// each have to actually flip something observable rather than the two
    /// calls coincidentally leaving behavior unchanged: succeeds before
    /// either call, fails once refused, succeeds again once healed.
    #[test]
    fn refuse_append_and_heal_are_each_independently_observable() {
        let fault = FaultGroupMetadataLog::new(FakeGroupMetadataLog::new());

        block_on(fault.append(&[entry_at(1, 10)])).expect("succeeds before any refusal");

        fault.refuse_append();
        let refused = block_on(fault.append(&[entry_at(2, 20)]));
        assert!(
            matches!(refused, Err(Error::Transient)),
            "append must be refused once refuse_append is called, got {refused:?}"
        );

        fault.heal();
        block_on(fault.append(&[entry_at(2, 20)])).expect("succeeds again once healed");
    }
}
