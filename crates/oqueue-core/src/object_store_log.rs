//! A [`MetadataLog`] that survives the process: its entries live in object
//! storage (`ADR-0046`, `M6.1`).
//!
//! ⚠️ **One segment object per `append`, written only if absent.** Segments
//! are keyed by a contiguous sequence number, so the conditional write is both
//! the durability point — an append is durable exactly when its PUT returns —
//! and the fence: two writers that both believe they own the log race for one
//! sequence number, one PUT lands, and the other is refused and stays refused,
//! because its idea of the next number is now permanently stale.
//!
//! ⚠️ **Opened without LIST** (`mission.md`): read the base object, then GET
//! segments forward from it until one is absent. The entries are held in
//! memory once read, so `read_from` costs no request; how many there are is
//! bounded by pruning below the newest snapshot (`M6.5`).
//!
//! ⚠️ **An append whose PUT landed but whose answer was lost is reported as a
//! failure and is nonetheless durable.** Every object store has this ambiguity.
//! It is safe here in the direction that matters: the caller acknowledges
//! nothing for a refused append, and this log's next append targets the same
//! sequence number, finds it taken, and is refused — so a writer that lost an
//! answer stops rather than writing past an entry it does not know it wrote.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{
    BoxFuture, ByteRange, CommitVersion, Error, GroupMetadataEntry, GroupMetadataLog,
    MetadataEntry, MetadataLog, ObjectKey, ObjectStore, Precondition, Result, decode_segment,
    encode_segment, group_segment,
};

/// The bytes a base object begins with.
const BASE_MAGIC: [u8; 4] = *b"OQMB";

/// How one log's entries become a segment's bytes and back.
trait SegmentFormat {
    type Entry: Clone + Send + Sync;
    fn encode(entries: &[Self::Entry]) -> Result<Vec<u8>>;
    fn decode(bytes: &[u8]) -> Result<Vec<Self::Entry>>;
    fn version(entry: &Self::Entry) -> CommitVersion;
}

/// The coordinator's metadata log (`M6.1`).
struct Metadata;

impl SegmentFormat for Metadata {
    type Entry = MetadataEntry;
    fn encode(entries: &[MetadataEntry]) -> Result<Vec<u8>> {
        encode_segment(entries)
    }
    fn decode(bytes: &[u8]) -> Result<Vec<MetadataEntry>> {
        decode_segment(bytes)
    }
    fn version(entry: &MetadataEntry) -> CommitVersion {
        entry.version()
    }
}

/// The group metadata log (`M6.6`).
struct Group;

impl SegmentFormat for Group {
    type Entry = GroupMetadataEntry;
    fn encode(entries: &[GroupMetadataEntry]) -> Result<Vec<u8>> {
        group_segment::encode(entries)
    }
    fn decode(bytes: &[u8]) -> Result<Vec<GroupMetadataEntry>> {
        group_segment::decode(bytes)
    }
    fn version(entry: &GroupMetadataEntry) -> CommitVersion {
        entry.version()
    }
}

/// One log's segments under one prefix, whichever records they hold.
struct Segments<F: SegmentFormat> {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    state: Mutex<State<F::Entry>>,
}

/// What has been read or appended, and where the next segment goes.
struct State<E> {
    next_seq: u64,
    entries: Vec<E>,
}

impl<F: SegmentFormat> Segments<F> {
    async fn open(store: Arc<dyn ObjectStore>, prefix: String) -> Result<Self> {
        let base = read_base(&*store, &prefix).await?;
        let mut entries: Vec<F::Entry> = Vec::new();
        let mut seq = base;
        loop {
            let bytes = match store
                .get(&segment_key(&prefix, seq)?, ByteRange::Full)
                .await
            {
                Ok(bytes) => bytes,
                Err(Error::ObjectNotFound { .. }) => break,
                Err(other) => return Err(other),
            };
            let segment = F::decode(&bytes)?;
            validate::<F>(&segment, entries.last().map(F::version))?;
            entries.extend(segment);
            seq = next(seq)?;
        }
        Ok(Self {
            store,
            prefix,
            state: Mutex::new(State {
                next_seq: seq,
                entries,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State<F::Entry>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn append(&self, entries: &[F::Entry]) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        // ⚠️ Read under the lock, written without it (`async-concurrency.md`
        // rule 6): the conditional PUT is what serializes two appends, so no
        // lock is held across its await.
        let (seq, last) = {
            let state = self.lock();
            (state.next_seq, state.entries.last().map(F::version))
        };
        validate::<F>(entries, last)?;
        let payload = F::encode(entries)?;
        let key = segment_key(&self.prefix, seq)?;
        self.store
            .put(&key, payload, Some(Precondition::IfAbsent))
            .await?;
        let mut state = self.lock();
        state.next_seq = next(seq)?;
        state.entries.extend_from_slice(entries);
        drop(state);
        Ok(())
    }

    fn read_from(&self, start: CommitVersion, max_entries: usize) -> Vec<F::Entry> {
        self.lock()
            .entries
            .iter()
            .filter(|entry| F::version(entry) >= start)
            .take(max_entries)
            .cloned()
            .collect()
    }

    fn last_version(&self) -> Option<CommitVersion> {
        self.lock().entries.last().map(F::version)
    }

    fn describe(&self, name: &str, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let state = self.lock();
        f.debug_struct(name)
            .field("prefix", &self.prefix)
            .field("next_seq", &state.next_seq)
            .field("entries", &state.entries.len())
            .finish_non_exhaustive()
    }
}

/// A metadata log whose segments are objects under one prefix.
pub struct ObjectStoreMetadataLog(Segments<Metadata>);

/// ⚠️ Renders the count and the prefix, never the records — the rule every
/// `MetadataLog` in this crate follows.
impl core::fmt::Debug for ObjectStoreMetadataLog {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.describe("ObjectStoreMetadataLog", f)
    }
}

impl ObjectStoreMetadataLog {
    /// Opens the log under `prefix`, reading every live segment.
    ///
    /// ⚠️ **An empty prefix is an empty log**, not an error: the first
    /// coordinator of a shard opens it this way.
    ///
    /// # Errors
    ///
    /// The store's own error for a read that fails other than as absent;
    /// [`Error::MalformedMetadataSegment`] for a base or segment that cannot
    /// be read whole; and [`Error::NonMonotonicCommitVersion`] if the segments
    /// do not form one increasing line.
    pub async fn open(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Result<Self> {
        Ok(Self(Segments::open(store, prefix.into()).await?))
    }
}

impl MetadataLog for ObjectStoreMetadataLog {
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.0.append(entries))
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<MetadataEntry>>> {
        Box::pin(async move { Ok(self.0.read_from(start, max_entries)) })
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        Box::pin(async move { Ok(self.0.last_version()) })
    }
}

/// A group metadata log whose segments are objects under one prefix
/// (`M6.6`): committed consumer offsets and group transitions, durable the
/// way the metadata log is (`ADR-0046`).
pub struct ObjectStoreGroupMetadataLog(Segments<Group>);

/// ⚠️ Renders the count and the prefix, never a group or an offset.
impl core::fmt::Debug for ObjectStoreGroupMetadataLog {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.describe("ObjectStoreGroupMetadataLog", f)
    }
}

impl ObjectStoreGroupMetadataLog {
    /// Opens the group log under `prefix`, reading every live segment.
    ///
    /// # Errors
    ///
    /// As [`ObjectStoreMetadataLog::open`].
    pub async fn open(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Result<Self> {
        Ok(Self(Segments::open(store, prefix.into()).await?))
    }
}

impl GroupMetadataLog for ObjectStoreGroupMetadataLog {
    fn append<'a>(&'a self, entries: &'a [GroupMetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(self.0.append(entries))
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<GroupMetadataEntry>>> {
        Box::pin(async move { Ok(self.0.read_from(start, max_entries)) })
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        Box::pin(async move { Ok(self.0.last_version()) })
    }
}

/// Refuses a batch whose versions do not strictly increase from `last` —
/// guarantee 3, the check both fakes make.
fn validate<F: SegmentFormat>(entries: &[F::Entry], last: Option<CommitVersion>) -> Result<()> {
    let mut previous = last;
    for entry in entries {
        let version = F::version(entry);
        if let Some(prev) = previous
            && version <= prev
        {
            return Err(Error::NonMonotonicCommitVersion {
                expected_above: prev.get(),
                got: version.get(),
            });
        }
        previous = Some(version);
    }
    Ok(())
}

fn next(seq: u64) -> Result<u64> {
    seq.checked_add(1).ok_or(Error::CommitVersionOverflow {
        base: seq,
        delta: 1,
    })
}

/// The key of segment `seq` under `prefix`.
///
/// ⚠️ **Zero-padded to twenty digits**, `u64::MAX`'s width, so keys sort in
/// sequence order for anyone who does list them — an operator, an inventory.
fn segment_key(prefix: &str, seq: u64) -> Result<ObjectKey> {
    ObjectKey::new(format!("{prefix}/log/{seq:020}"))
}

/// The lowest live segment, from the base object; zero when there is none.
async fn read_base(store: &dyn ObjectStore, prefix: &str) -> Result<u64> {
    let key = ObjectKey::new(format!("{prefix}/base"))?;
    let bytes = match store.get(&key, ByteRange::Full).await {
        Ok(bytes) => bytes,
        Err(Error::ObjectNotFound { .. }) => return Ok(0),
        Err(other) => return Err(other),
    };
    let malformed = Error::MalformedMetadataSegment { at: 0 };
    let (magic, seq) = bytes.split_at_checked(4).ok_or_else(|| malformed.clone())?;
    if magic != BASE_MAGIC {
        return Err(malformed);
    }
    let seq: [u8; 8] = seq.try_into().map_err(|_| malformed)?;
    Ok(u64::from_be_bytes(seq))
}
