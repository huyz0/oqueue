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
//! memory once read, so `read_from` costs no request. ⚠️ **How many there
//! are is not bounded**: pruning bounds the segment GETs an open costs, but a
//! snapshot holds every entry and this log keeps them all in memory, so both
//! grow with the log's age — `M6.18`, handed on at M6's close.
//!
//! ⚠️ **An append whose PUT landed but whose answer was lost is reported as a
//! failure and is nonetheless durable.** Every object store has this ambiguity.
//! It is safe here in the direction that matters: the caller acknowledges
//! nothing for a refused append, and this log's next append targets the same
//! sequence number, finds it taken, and is refused — so a writer that lost an
//! answer stops rather than writing past an entry it does not know it wrote.

mod base;

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use base::{Base, base_key, read_base};

use crate::{
    BoxFuture, ByteRange, CommitVersion, Error, GroupMetadataEntry, GroupMetadataLog,
    MetadataEntry, MetadataLog, ObjectKey, ObjectStore, Precondition, Result, decode_segment,
    encode_segment, group_segment,
};

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
    /// The newest base generation, `None` before the first checkpoint.
    generation: Option<u64>,
    /// The lowest live segment.
    base_seq: u64,
    /// The snapshot the base names, if any.
    snapshot: Option<ObjectKey>,
    /// Segment bytes appended since the last checkpoint — its trigger.
    unsnapshotted_bytes: u64,
}

impl<F: SegmentFormat> Segments<F> {
    async fn open(store: Arc<dyn ObjectStore>, prefix: String) -> Result<Self> {
        // ⚠️ **A walk that raced a checkpoint is started over** (`M6.4`'s
        // review). Pruning deletes segment keys, so a segment deleted under a
        // reader reads as absent — as the tail — and an append there would be
        // written behind the newer base, where no later open looks. A
        // checkpoint commits its base generation *before* it deletes, so a
        // reader that met a deleted segment always finds the base moved when
        // it looks again, and reads from the new one.
        loop {
            let (generation, base) = read_base(&*store, &prefix).await?;
            let (entries, seq) = walk::<F>(&*store, &prefix, &base).await?;
            if read_base(&*store, &prefix).await?.0 != generation {
                continue;
            }
            return Ok(Self {
                store,
                prefix,
                state: Mutex::new(State {
                    next_seq: seq,
                    entries,
                    generation,
                    base_seq: base.seq,
                    snapshot: base.snapshot,
                    unsnapshotted_bytes: 0,
                }),
            });
        }
    }

    /// Folds every live segment into one snapshot object, points a new base
    /// at it, and deletes what the snapshot replaced (`M6.4`, `M6.5`).
    ///
    /// ⚠️ **Ordered so a crash anywhere leaves a readable log**: the snapshot
    /// is written first (unreferenced until the base names it), then the base
    /// generation (the commit point), and only then are the old segments and
    /// snapshot deleted. A crash before the base leaves an orphan object; a
    /// crash after it leaves segments the new base no longer reads.
    ///
    /// ⚠️ **A writer that is no longer the log's tail refuses.** Before the
    /// base is written it checks its own next segment is still absent; one
    /// that is present means another writer appended, and this one's view is
    /// stale. The base generation is itself written only if absent, so two
    /// checkpointers cannot both commit one generation.
    async fn checkpoint(&self) -> Result<bool> {
        let (seq, generation, old_base, old_snapshot, payload, last) = {
            let state = self.lock();
            let Some(last) = state.entries.last().map(F::version) else {
                return Ok(false);
            };
            if state.next_seq == state.base_seq {
                return Ok(false);
            }
            (
                state.next_seq,
                state.generation.map_or(Ok(0), next)?,
                state.base_seq,
                state.snapshot.clone(),
                F::encode(&state.entries)?,
                last,
            )
        };
        let snapshot = ObjectKey::new(format!("{}/snap/{:020}", self.prefix, last.get()))?;
        // ⚠️ Already present is fine: a retry after a crash wrote the same
        // entries under the same key.
        put_if_absent(&*self.store, &snapshot, payload).await?;
        // ⚠️ The tail *now*, not the one captured with the snapshot: this
        // process's own appends since then are live segments past the new
        // base, not another writer (`M6.4`'s review).
        let tail = self.lock().next_seq;
        self.still_the_tail(tail).await?;
        let base = Base {
            seq,
            snapshot: Some(snapshot.clone()),
        };
        self.store
            .put(
                &base_key(&self.prefix, generation)?,
                base.encode()?,
                Some(Precondition::IfAbsent),
            )
            .await?;
        {
            let mut state = self.lock();
            state.generation = Some(generation);
            state.base_seq = seq;
            state.snapshot = Some(snapshot.clone());
            state.unsnapshotted_bytes = 0;
        }
        let mut retired = (old_base..seq)
            .map(|old| segment_key(&self.prefix, old))
            .collect::<Result<Vec<_>>>()?;
        retired.extend(old_snapshot.filter(|old| *old != snapshot));
        self.store.delete(&retired).await?;
        Ok(true)
    }

    /// Refuses when segment `seq` exists: another writer has appended past
    /// this log's view of the tail.
    async fn still_the_tail(&self, seq: u64) -> Result<()> {
        let tail = segment_key(&self.prefix, seq)?;
        match self.store.get(&tail, ByteRange::Full).await {
            Err(Error::ObjectNotFound { .. }) => Ok(()),
            Ok(_) => Err(Error::PreconditionFailed { key: tail }),
            Err(other) => Err(other),
        }
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
        let size = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        let key = segment_key(&self.prefix, seq)?;
        self.store
            .put(&key, payload, Some(Precondition::IfAbsent))
            .await?;
        let mut state = self.lock();
        state.next_seq = next(seq)?;
        state.entries.extend_from_slice(entries);
        state.unsnapshotted_bytes = state.unsnapshotted_bytes.saturating_add(size);
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

/// A segment log opened on first use (`M6.10`): building one touches no
/// store, so a node whose store is unreachable at boot still gets a log, and
/// every operation opens it until one open succeeds.
struct Lazy<F: SegmentFormat> {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    opened: OnceLock<Segments<F>>,
}

impl<F: SegmentFormat> Lazy<F> {
    const fn new(store: Arc<dyn ObjectStore>, prefix: String) -> Self {
        Self {
            store,
            prefix,
            opened: OnceLock::new(),
        }
    }

    /// The opened log, opening it now if no earlier call has.
    ///
    /// ⚠️ **Two racing first calls may both open**, and the second's result is
    /// discarded: opening only reads, so the only cost is the duplicate GETs.
    async fn get(&self) -> Result<&Segments<F>> {
        if let Some(opened) = self.opened.get() {
            return Ok(opened);
        }
        let opened = Segments::open(Arc::clone(&self.store), self.prefix.clone()).await?;
        let _ = self.opened.set(opened);
        self.opened
            .get()
            .ok_or(Error::MalformedMetadataSegment { at: 0 })
    }

    fn describe(&self, name: &str, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.opened.get() {
            Some(opened) => opened.describe(name, f),
            None => f
                .debug_struct(name)
                .field("prefix", &self.prefix)
                .field("opened", &false)
                .finish_non_exhaustive(),
        }
    }
}

/// A metadata log whose segments are objects under one prefix.
pub struct ObjectStoreMetadataLog(Lazy<Metadata>);

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
        let log = Self::deferred(store, prefix);
        log.0.get().await?;
        Ok(log)
    }

    /// The log under `prefix`, opened on its first use rather than now
    /// (`M6.10`): a node whose store is unreachable at boot still starts, and
    /// each operation retries the open until one succeeds.
    #[must_use]
    pub fn deferred(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self(Lazy::new(store, prefix.into()))
    }

    /// Writes a snapshot of every live entry and prunes what it replaces;
    /// `Ok(false)` when there was nothing to fold. See `Segments::checkpoint`.
    ///
    /// # Errors
    ///
    /// The store's error, or [`Error::PreconditionFailed`] when this log is no
    /// longer the tail — another writer has appended since it last looked.
    pub async fn checkpoint(&self) -> Result<bool> {
        self.0.get().await?.checkpoint().await
    }

    /// Segment bytes appended since the last checkpoint: the trigger a
    /// checkpoint cadence reads, so recovery time tracks journal volume and
    /// not the wall clock (`M6.md` task 6).
    #[must_use]
    pub fn unsnapshotted_bytes(&self) -> u64 {
        self.0
            .opened
            .get()
            .map_or(0, |opened| opened.lock().unsnapshotted_bytes)
    }
}

impl MetadataLog for ObjectStoreMetadataLog {
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.0.get().await?.append(entries).await })
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<MetadataEntry>>> {
        Box::pin(async move { Ok(self.0.get().await?.read_from(start, max_entries)) })
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        Box::pin(async move { Ok(self.0.get().await?.last_version()) })
    }
}

/// A group metadata log whose segments are objects under one prefix
/// (`M6.6`): committed consumer offsets and group transitions, durable the
/// way the metadata log is (`ADR-0046`).
pub struct ObjectStoreGroupMetadataLog(Lazy<Group>);

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
        let log = Self::deferred(store, prefix);
        log.0.get().await?;
        Ok(log)
    }

    /// The log under `prefix`, opened on its first use rather than now
    /// (`M6.10`): a node whose store is unreachable at boot still starts, and
    /// each operation retries the open until one succeeds.
    #[must_use]
    pub fn deferred(store: Arc<dyn ObjectStore>, prefix: impl Into<String>) -> Self {
        Self(Lazy::new(store, prefix.into()))
    }
}

impl GroupMetadataLog for ObjectStoreGroupMetadataLog {
    fn append<'a>(&'a self, entries: &'a [GroupMetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.0.get().await?.append(entries).await })
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<GroupMetadataEntry>>> {
        Box::pin(async move { Ok(self.0.get().await?.read_from(start, max_entries)) })
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        Box::pin(async move { Ok(self.0.get().await?.last_version()) })
    }
}

/// Reads a base's snapshot and every segment after it, until one is absent;
/// the entries, and the sequence number of the first absent segment.
async fn walk<F: SegmentFormat>(
    store: &dyn ObjectStore,
    prefix: &str,
    base: &Base,
) -> Result<(Vec<F::Entry>, u64)> {
    let mut entries: Vec<F::Entry> = Vec::new();
    if let Some(snapshot) = &base.snapshot {
        entries = F::decode(&store.get(snapshot, ByteRange::Full).await?)?;
        validate::<F>(&entries, None)?;
    }
    let mut seq = base.seq;
    loop {
        let bytes = match store.get(&segment_key(prefix, seq)?, ByteRange::Full).await {
            Ok(bytes) => bytes,
            Err(Error::ObjectNotFound { .. }) => return Ok((entries, seq)),
            Err(other) => return Err(other),
        };
        let segment = F::decode(&bytes)?;
        validate::<F>(&segment, entries.last().map(F::version))?;
        entries.extend(segment);
        seq = next(seq)?;
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

async fn put_if_absent(store: &dyn ObjectStore, key: &ObjectKey, payload: Vec<u8>) -> Result<()> {
    match store.put(key, payload, Some(Precondition::IfAbsent)).await {
        Ok(_) | Err(Error::PreconditionFailed { .. }) => Ok(()),
        Err(other) => Err(other),
    }
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
