//! The group-metadata-log seam, and the in-memory fake that makes it
//! testable — `M4.14`, `ADR-0035`.
//!
//! ⚠️ **`MetadataLog`'s own shape, verbatim, for an unrelated domain.**
//! `ADR-0035` is the record of why this is its own trait rather than a
//! generalization of `MetadataLog` or a new `MetadataRecord` variant — read
//! it before changing either seam to reach for the other.

use crate::{BoxFuture, CommitVersion, Error, GroupMetadataRecord, Result};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// One record at the position it was committed at.
///
/// ⚠️ The version is assigned by the caller, not by the log —
/// `MetadataEntry`'s own doc, the same reason: a single allocator per log
/// makes the append the serialization point, and a log that assigned
/// versions itself would be a second allocator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMetadataEntry {
    version: CommitVersion,
    record: GroupMetadataRecord,
}

impl GroupMetadataEntry {
    /// Pairs a record with the version it commits at.
    #[must_use]
    pub const fn new(version: CommitVersion, record: GroupMetadataRecord) -> Self {
        Self { version, record }
    }

    /// The position this record occupies.
    #[must_use]
    pub const fn version(&self) -> CommitVersion {
        self.version
    }

    /// What happened.
    #[must_use]
    pub const fn record(&self) -> &GroupMetadataRecord {
        &self.record
    }
}

/// Durably appends group metadata records in `CommitVersion` order, and
/// reads them back.
///
/// ⚠️ **This is the seam, and the engine behind it is deliberately
/// undecided** — `MetadataLog`'s own doc, unchanged reasoning: the trait
/// exists first so `M4.14`'s callers can be written and tested against
/// something, and so the engine choice (the same one `MetadataLog` is
/// waiting on, doc 10 #12, `M6`) stays an implementation task.
///
/// # What an implementor must guarantee
///
/// The same five guarantees `MetadataLog` documents, restated for this
/// seam:
///
/// 1. **Durability.** When `append`'s future resolves `Ok`, the entries are
///    durable.
/// 2. **Strictly increasing versions.** A batch that would break it is
///    refused with [`Error::NonMonotonicCommitVersion`].
/// 3. ⚠️ **A refused append stores nothing** — not a prefix.
/// 4. **`read_from` is inclusive and ordered**, returns at most
///    `max_entries`, and is empty rather than an error past the end.
/// 5. ⚠️ **A short page means the log has no more.**
pub trait GroupMetadataLog: Send + Sync + core::fmt::Debug {
    /// Durably appends a batch, in the order given.
    ///
    /// # Errors
    ///
    /// [`Error::NonMonotonicCommitVersion`] if the batch is not strictly
    /// increasing, or if its first entry is not above the last stored
    /// version. On this error the log is unchanged.
    fn append<'a>(&'a self, entries: &'a [GroupMetadataEntry]) -> BoxFuture<'a, Result<()>>;

    /// Reads up to `max_entries` entries from `start` inclusive, in order.
    ///
    /// # Errors
    ///
    /// Implementation-defined. The fake here does not fail.
    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<GroupMetadataEntry>>>;

    /// The highest version stored, or `None` if nothing has been appended.
    ///
    /// # Errors
    ///
    /// Implementation-defined. The fake here does not fail.
    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>>;
}

/// An in-memory [`GroupMetadataLog`], faithful to the documented contract.
///
/// ⚠️ **It is not a stand-in for durability** — `FakeMetadataLog`'s own
/// doc, unchanged reasoning: nothing here survives the process. A real
/// engine is `M6`'s to choose, the same one `MetadataLog` itself is
/// waiting on (`ADR-0035`).
pub struct FakeGroupMetadataLog {
    entries: Mutex<Vec<GroupMetadataEntry>>,
    /// How many times [`Self::read_from`] has been called — `Cluster`'s own
    /// `topic_lookups` shape, unchanged reasoning: a caller replaying this
    /// log pages against object storage in the real engine, so a caller
    /// that reads one page more than the data requires is a real, billable
    /// round trip and not just a mutation-testing artifact.
    read_from_calls: AtomicU64,
}

impl FakeGroupMetadataLog {
    /// An empty log.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            read_from_calls: AtomicU64::new(0),
        }
    }

    /// How many entries it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// How many times [`GroupMetadataLog::read_from`] has been called —
    /// a page-budget assertion for a caller that replays this log, not a
    /// property of the trait itself.
    #[must_use]
    pub fn read_from_calls(&self) -> u64 {
        self.read_from_calls.load(Ordering::Relaxed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<GroupMetadataEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Validates the whole batch before storing any of it — guarantee 3.
    fn validate(entries: &[GroupMetadataEntry], last: Option<CommitVersion>) -> Result<()> {
        let mut previous = last;
        for entry in entries {
            if let Some(prev) = previous
                && entry.version() <= prev
            {
                return Err(Error::NonMonotonicCommitVersion {
                    expected_above: prev.get(),
                    got: entry.version().get(),
                });
            }
            previous = Some(entry.version());
        }
        Ok(())
    }
}

impl Default for FakeGroupMetadataLog {
    fn default() -> Self {
        Self::new()
    }
}

/// ⚠️ Renders the count and never the records — `FakeMetadataLog`'s own
/// rule, unchanged reasoning.
impl core::fmt::Debug for FakeGroupMetadataLog {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FakeGroupMetadataLog")
            .field("entries", &self.len())
            .finish()
    }
}

/// ⚠️ **Every method does its work inside the future it returns, never when
/// it is called** — `FakeMetadataLog`'s own precedent, unchanged reasoning.
impl GroupMetadataLog for FakeGroupMetadataLog {
    fn append<'a>(&'a self, entries: &'a [GroupMetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut held = self.lock();
            Self::validate(entries, held.last().map(GroupMetadataEntry::version))?;
            held.extend_from_slice(entries);
            drop(held);
            Ok(())
        })
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<GroupMetadataEntry>>> {
        Box::pin(async move {
            self.read_from_calls.fetch_add(1, Ordering::Relaxed);
            let held = self.lock();
            let page: Vec<GroupMetadataEntry> = held
                .iter()
                .filter(|entry| entry.version() >= start)
                .take(max_entries)
                .cloned()
                .collect();
            drop(held);
            Ok(page)
        })
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        Box::pin(async move {
            let held = self.lock();
            let last = held.last().map(GroupMetadataEntry::version);
            drop(held);
            Ok(last)
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{FakeGroupMetadataLog, GroupMetadataEntry, GroupMetadataLog};
    use crate::test_executor::block_on;
    use crate::{CommitVersion, Error, GroupId, GroupMetadataRecord, TopicId};

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

    #[test]
    fn len_and_is_empty_reflect_what_was_appended() {
        let log = FakeGroupMetadataLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);

        block_on(log.append(&[entry_at(1, 10), entry_at(2, 20)])).expect("first append succeeds");

        assert!(!log.is_empty());
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn debug_renders_the_actual_count() {
        let log = FakeGroupMetadataLog::new();
        assert!(
            format!("{log:?}").contains('0'),
            "an empty log's Debug output should name a count of 0"
        );

        block_on(log.append(&[entry_at(1, 10), entry_at(2, 20), entry_at(3, 30)]))
            .expect("append succeeds");

        let rendered = format!("{log:?}");
        assert!(
            rendered.contains('3'),
            "Debug output {rendered:?} should name the count 3, not stay empty"
        );
    }

    #[test]
    fn validate_rejects_a_version_no_higher_than_the_last_stored_one() {
        let log = FakeGroupMetadataLog::new();
        block_on(log.append(&[entry_at(5, 1)])).expect("first append succeeds");

        // Exactly equal to the last stored version — the `<=` boundary
        // itself, not just "less than".
        let rejected = block_on(log.append(&[entry_at(5, 2)]));
        assert!(
            matches!(rejected, Err(Error::NonMonotonicCommitVersion { .. })),
            "a version equal to the last stored one must be refused, got {rejected:?}"
        );
        assert_eq!(log.len(), 1, "a refused append must store nothing");

        // Strictly above it is accepted.
        block_on(log.append(&[entry_at(6, 3)])).expect("strictly-increasing append succeeds");
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn read_from_includes_the_start_version_and_excludes_before_it() {
        let log = FakeGroupMetadataLog::new();
        block_on(log.append(&[entry_at(1, 10), entry_at(2, 20), entry_at(3, 30)]))
            .expect("append succeeds");

        let page = block_on(log.read_from(CommitVersion::new(2), 10)).expect("read succeeds");
        let versions: Vec<u64> = page.iter().map(|e| e.version().get()).collect();
        assert_eq!(
            versions,
            vec![2, 3],
            "read_from(2) must include version 2 itself and exclude version 1"
        );
    }

    #[test]
    fn read_from_calls_counts_every_call_not_a_fixed_number() {
        let log = FakeGroupMetadataLog::new();
        assert_eq!(log.read_from_calls(), 0, "nothing has been read yet");

        block_on(log.read_from(CommitVersion::ZERO, 10)).expect("read succeeds");
        assert_eq!(log.read_from_calls(), 1, "one call must register as one");

        block_on(log.read_from(CommitVersion::ZERO, 10)).expect("read succeeds");
        block_on(log.read_from(CommitVersion::ZERO, 10)).expect("read succeeds");
        assert_eq!(
            log.read_from_calls(),
            3,
            "a third call must move the count past both 0 and 1"
        );
    }
}
