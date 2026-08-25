//! The metadata-log seam, and the in-memory fake that makes it testable.

use crate::{BoxFuture, CommitVersion, Error, MetadataRecord, Result};
use std::sync::Mutex;

/// One record at the position the coordinator stamped it with.
///
/// ⚠️ The version is assigned by the caller, not by the log. `ADR-0020` puts a
/// single allocator per log and makes the append the serialization point, so
/// the log's job is to *enforce* the order it is handed, never to invent one —
/// a log that assigned versions itself would be a second allocator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataEntry {
    version: CommitVersion,
    record: MetadataRecord,
}

impl MetadataEntry {
    /// Pairs a record with the version it commits at.
    #[must_use]
    pub const fn new(version: CommitVersion, record: MetadataRecord) -> Self {
        Self { version, record }
    }

    /// The position this record occupies.
    #[must_use]
    pub const fn version(&self) -> CommitVersion {
        self.version
    }

    /// What happened.
    #[must_use]
    pub const fn record(&self) -> &MetadataRecord {
        &self.record
    }
}

/// Durably appends metadata records in `CommitVersion` order, and reads them
/// back.
///
/// ⚠️ **This is the seam, and the engine behind it is deliberately undecided.**
/// `ADR-0020` point 5 leaves doc 10 #12 — `SQLite`, `redb`, `RocksDB`,
/// `fjall` or `SlateDB` — to a benchmark doc 13 §6 asks for and that has not
/// been run. The
/// trait exists first so `M3`'s coordinator can be written and tested against
/// something, and so the engine choice stays an implementation task rather
/// than an architecture nobody chose.
///
/// # What an implementor must guarantee
///
/// 1. **Durability.** When `append`'s future resolves `Ok`, the entries are
///    durable. Nothing above this seam re-checks, because `ADR-0020`'s
///    assign → journal → ack ordering makes this the moment an offset becomes
///    externally visible.
/// 2. **Strictly increasing versions.** Entries are appended in strictly
///    increasing `CommitVersion` order, within a batch and across batches. A
///    batch that would break it is refused with
///    [`Error::NonMonotonicCommitVersion`] (`M3.md` task 14).
/// 3. ⚠️ **A refused append stores nothing.** Not a prefix, not the entries
///    before the offending one. A caller that retries after a rejection would
///    otherwise replay onto a log already holding part of the batch, and the
///    fold that derives offsets would double-count silently.
/// 4. **`read_from` is inclusive and ordered**, returns at most
///    `max_entries`, and is empty rather than an error past the end.
/// 5. ⚠️ **A short page means the log has no more**, and is never a partial
///    answer an implementation returned for its own convenience — a block
///    boundary, a page split, an internal limit. A reader is entitled to stop
///    on one: `oqueue-index`'s applier does, which is what saves it a round
///    trip per catch-up. An engine that answered `400` of a requested `1,000`
///    with `5,000` still available would leave that applier believing it had
///    caught up, with the index behind the log and **no error anywhere** —
///    the silent-staleness class `M3.md`'s Risks section is about. Return
///    fewer only when there are fewer.
///
/// ⚠️ **Ordering is per log, and a log is per metadata shard** (`ADR-0020`).
/// Versions from two logs are not comparable; the shard identifier that would
/// make that checkable is `M7`'s.
pub trait MetadataLog: Send + Sync + core::fmt::Debug {
    /// Durably appends a batch, in the order given.
    ///
    /// An empty batch is accepted and changes nothing.
    ///
    /// # Errors
    ///
    /// [`Error::NonMonotonicCommitVersion`] if the batch is not strictly
    /// increasing, or if its first entry is not above the last stored
    /// version. ⚠️ On this error the log is unchanged.
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>>;

    /// Reads up to `max_entries` entries from `start` inclusive, in order.
    ///
    /// # Errors
    ///
    /// Implementation-defined. The fake here does not fail.
    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<MetadataEntry>>>;

    /// The highest version stored, or `None` if nothing has been appended.
    ///
    /// ⚠️ `None` is distinct from `Some(CommitVersion::ZERO)`: zero is a
    /// position a real entry can occupy.
    ///
    /// # Errors
    ///
    /// Implementation-defined. The fake here does not fail.
    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>>;
}

/// An in-memory [`MetadataLog`], faithful to the documented contract.
///
/// ⚠️ **It is not a stand-in for durability.** It satisfies guarantee 1
/// vacuously — nothing here survives the process — so a test that means to
/// exercise crash behaviour cannot get it from this type.
///
/// ⚠️ And **no `M3` row covers that gap for this seam.** `M3.15`'s durability
/// conformance case is [`ObjectStore`](crate::ObjectStore)'s
/// (`crash_after_put_before_ack`), not this trait's. Guarantee 1 becomes
/// checkable only when a real engine exists to check it against, which is
/// doc 10 #12 — still open, and `M6`'s to close. Until then the guarantee is
/// stated and unenforced, which is worth knowing rather than discovering.
pub struct FakeMetadataLog {
    entries: Mutex<Vec<MetadataEntry>>,
}

impl FakeMetadataLog {
    /// An empty log.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<MetadataEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Validates the whole batch before storing any of it — guarantee 3.
    fn validate(entries: &[MetadataEntry], last: Option<CommitVersion>) -> Result<()> {
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

impl Default for FakeMetadataLog {
    fn default() -> Self {
        Self::new()
    }
}

/// ⚠️ Renders the count and never the records. A `Debug` line in a test
/// failure must not become a transcript of the log — same rule
/// [`FakeObjectStore`](crate::FakeObjectStore) follows for payloads.
impl core::fmt::Debug for FakeMetadataLog {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FakeMetadataLog")
            .field("entries", &self.len())
            .finish()
    }
}

/// ⚠️ **Every method does its work inside the future it returns, never when it
/// is called** — the same shape [`FakeObjectStore`](crate::FakeObjectStore)
/// uses, and it is guarantee 1 that requires it. A fake that mutated on call
/// would make `append` durable at a moment the trait says it is not: a caller
/// that builds the future and drops it unpolled — a `select!` losing to a
/// deadline is the ordinary way that happens — would find the batch stored
/// and `last_version` advanced against this fake, and nothing stored against
/// a real engine. The retry then takes opposite paths on the two, which is
/// precisely the divergence a fake exists to rule out.
///
/// The lock is taken in a tight inner scope rather than across the whole
/// block, which is what keeps the guard's lifetime obvious and satisfies
/// `clippy::significant_drop_tightening`.
impl MetadataLog for FakeMetadataLog {
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut held = self.lock();
            // ⚠️ Validated and written under one lock acquisition — never a
            // separate check followed by a separate write, which is the gap a
            // second racing appender could land in.
            Self::validate(entries, held.last().map(MetadataEntry::version))?;
            held.extend_from_slice(entries);
            drop(held);
            Ok(())
        })
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<MetadataEntry>>> {
        Box::pin(async move {
            let held = self.lock();
            let page: Vec<MetadataEntry> = held
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
            let last = held.last().map(MetadataEntry::version);
            drop(held);
            Ok(last)
        })
    }
}
