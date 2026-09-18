//! Retention: which partitions' data has outlived its retention, found from
//! the index alone.
//!
//! ⚠️ **FR-33's "including on partitions nobody is writing to"** is the whole
//! reason this is a round rather than a check on the write path. A produce-time
//! check never runs on the partition that most needs it, and a round that walked
//! every partition would cost a pass over every partition a node holds each
//! time. `ADR-0036`'s expiry heap is the third option: a min-heap keyed on when
//! each partition's data expires, so a round pops only what is due and touches
//! nothing else.
//!
//! ⚠️ **Sans-I/O, and it decides; it does not act** (`M5.18`). A round returns
//! the `Trimmed` records a coordinator should journal. The trim is
//! metadata-only (`M5.19`); physical deletion is the object lifecycle's, once
//! liveness says every partition in an object is dead.
//!
//! ⚠️ **Whole-partition expiry, keyed on the *newest* commit, and that is a
//! narrowing of the row rather than an oversight.** `M5.18`'s row keys the
//! heap on `ts_min + retention`, which is when a partition's *oldest* data
//! could first expire. But the index holds one time extent per partition, not
//! per object (`M5.86`), so a round can only trim the whole partition — and
//! that is safe only once the *newest* data has aged out. Keyed on `ts_min`, a
//! busy partition's deadline would fire every round and find nothing it may
//! trim. Progressive trims of an active partition's oldest objects need
//! per-object times, which is `M5.16`'s half.

use core::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use oqueue_core::{MaterializedIndex, MetadataRecord, PartitionId, Result, Timestamp, TopicId};

/// How long a partition's data is kept after its newest commit.
///
/// ⚠️ **Kafka's own default, `log.retention.hours = 168`**, so a client that
/// never configured retention sees the retention it would see anywhere else.
/// ⚠️ **UNDERIVED as a product choice** — no requirement names a figure — and
/// per-topic retention is configuration this crate does not read. It is a
/// constant rather than a knob, per `AGENTS.md` non-negotiable 2.
///
/// ⚠️ **A literal, and 168 hours in milliseconds**: `7 * 24 * 60 * 60 *
/// 1_000` spelled as arithmetic was a set of mutants no test could tell apart
/// from each other except by restating the product, and `check-drift.sh`
/// pins the value either way.
pub const DEFAULT_RETENTION_MS: i64 = 604_800_000;

/// When each partition's data expires, ordered so a round pops only what is
/// due.
///
/// ⚠️ **Derived state, never a second source of truth** (`M5.18`). It is
/// rebuilt from the index on start and every deadline it holds is re-checked
/// against the index when it pops, so a partition written to after it was
/// pushed is re-armed at its new deadline rather than trimmed. A heap that was
/// believed instead of checked would reap acknowledged data the moment a
/// stale entry came due.
///
/// ⚠️ **One heap entry per partition, not one per commit.** A deadline only
/// ever moves *later* — it is the newest commit's time plus the retention, and
/// the fold's extent only widens — so an armed partition whose deadline has
/// moved is left where it is, and the pop re-arms it at its real deadline
/// once the old one comes due. [`track`](Self::track) can therefore be called
/// on every commit and the heap stays at one entry per partition. A first
/// version re-pushed on every later deadline and let the old entry go stale,
/// which at a thousand commits a second and a seven-day retention is ~600M
/// entries per partition before the first stale one pops. Found by `M5.18`'s
/// second round.
///
/// ⚠️ **The caller re-arms a partition after each commit to it**, by calling
/// `track`. A trimmed partition leaves the heap — its data is gone and there
/// is no deadline to hold — and a later write gives it one again only if
/// someone says so. The index cannot: nothing in `MaterializedIndex` notifies,
/// and the heap is deliberately not a second fold of the log. Without it, a
/// partition trimmed once and written again would never expire again, which is
/// the idle case FR-33 exists for. Found by `M5.18`'s first round.
#[derive(Debug, Default)]
pub struct ExpiryHeap {
    due: BinaryHeap<Reverse<(i64, TopicId, PartitionId)>>,
    armed: HashMap<(TopicId, PartitionId), i64>,
    retention_ms: i64,
    popped: usize,
}

impl ExpiryHeap {
    /// An empty heap for partitions retained `retention_ms` after their newest
    /// commit.
    #[must_use]
    pub fn new(retention_ms: i64) -> Self {
        Self {
            due: BinaryHeap::new(),
            armed: HashMap::new(),
            retention_ms,
            popped: 0,
        }
    }

    /// Builds the heap from what the index holds for `partitions`.
    ///
    /// ⚠️ **The partitions come from the caller**, for the reason `sweep.rs`
    /// gives: `MaterializedIndex` cannot enumerate what it holds, and the
    /// coordinator, which holds the catalog, is the caller.
    #[must_use]
    pub fn rebuilt<I>(index: &I, partitions: &[(TopicId, PartitionId)], retention_ms: i64) -> Self
    where
        I: MaterializedIndex + ?Sized,
    {
        let mut heap = Self::new(retention_ms);
        for (topic, partition) in partitions {
            heap.track(index, topic, *partition);
        }
        heap
    }

    /// Arms a partition that is not armed — call it after every commit to the
    /// partition.
    ///
    /// ⚠️ **An armed partition is left alone**, whatever its deadline has
    /// become: the deadline can only have moved later, and the pop re-arms it
    /// there. That is what keeps the heap at one entry per partition.
    pub fn track<I>(&mut self, index: &I, topic: &TopicId, partition: PartitionId)
    where
        I: MaterializedIndex + ?Sized,
    {
        let key = (topic.clone(), partition);
        if self.armed.contains_key(&key) {
            return;
        }
        if let Some(deadline) = self.deadline(index, topic, partition) {
            self.armed.insert(key, deadline);
            self.due.push(Reverse((deadline, topic.clone(), partition)));
        }
    }

    /// How many entries the heap holds — one per armed partition.
    #[must_use]
    pub fn len(&self) -> usize {
        self.due.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.due.is_empty()
    }

    /// Every partition's live deadline, earliest first — what a rebuild is
    /// compared against.
    #[must_use]
    pub fn deadlines(&self) -> Vec<(i64, TopicId, PartitionId)> {
        let mut all: Vec<_> = self
            .armed
            .iter()
            .map(|((topic, partition), deadline)| (*deadline, topic.clone(), *partition))
            .collect();
        all.sort();
        all
    }

    /// How many heap entries the last [`due`](Self::due) popped — what a round
    /// actually touched, rather than what it emitted.
    #[must_use]
    pub const fn popped_in_last_round(&self) -> usize {
        self.popped
    }

    /// The trims due at `now`.
    ///
    /// ⚠️ **Only what has come due is popped**, so a round over 100,000
    /// partitions with three expiring touches three — `ADR-0036`'s reason for
    /// a heap. A popped entry is acted on only if the index still agrees with
    /// its deadline: one written to since is re-armed at its later deadline,
    /// and one already trimmed to its end yields nothing.
    ///
    /// # Errors
    ///
    /// None today; `Result` so a caller's `?` does not change when a round
    /// learns to fail.
    pub fn due<I>(&mut self, index: &I, now: Timestamp) -> Result<Vec<MetadataRecord>>
    where
        I: MaterializedIndex + ?Sized,
    {
        let mut trims = Vec::new();
        // ⚠️ **Re-armed entries wait until the round ends**, so the loop is
        // bounded by what was in the heap when it started. Pushed straight
        // back, an entry whose recomputed deadline was still due would pop
        // again at once — and a loop whose only bound is that comparison is
        // one a mistake turns into a hang, which is what cargo-mutants found
        // when it flipped it (`M5.18`).
        let mut rearm: Vec<(TopicId, PartitionId)> = Vec::new();
        self.popped = 0;
        while let Some(Reverse((deadline, _, _))) = self.due.peek() {
            if *deadline > now.as_millis() {
                break;
            }
            let Some(Reverse((pushed, topic, partition))) = self.due.pop() else {
                break;
            };
            self.popped += 1;
            let key = (topic, partition);
            // ⚠️ Every entry is its partition's armed one — `track` never
            // pushes a second — so this is a guard, not a filter: an entry
            // that did not match would be a bookkeeping defect, and dropping
            // it keeps the round from trimming on a deadline nothing armed.
            if self.armed.get(&key) != Some(&pushed) {
                continue;
            }
            let (topic, partition) = key;
            if self.deadline(index, &topic, partition) == Some(pushed) {
                self.armed.remove(&(topic.clone(), partition));
                let end = index.end_offset(&topic, partition);
                if index.log_start(&topic, partition) < end {
                    trims.push(MetadataRecord::Trimmed {
                        topic,
                        partition,
                        start: end,
                    });
                }
            } else {
                // Written to since, and not re-armed by the caller.
                self.armed.remove(&(topic.clone(), partition));
                rearm.push((topic, partition));
            }
        }
        for (topic, partition) in rearm {
            self.track(index, &topic, partition);
        }
        Ok(trims)
    }

    /// When a partition's data expires, or `None` if it cannot be judged.
    ///
    /// ⚠️ **A partition whose newest commit is at the epoch is not judged**
    /// (`M5.86`'s second round). `WallClock` stamps `EPOCH` when the host's
    /// clock reads before 1970, and nothing makes that visible — so a
    /// partition whose every commit carries it is data whose age this round
    /// cannot know. Treating it as ancient would trim freshly acknowledged
    /// records on the first round after a misconfigured node wrote them, which
    /// is NFR-20's failure mode; skipping it keeps data a correct clock would
    /// have kept, which is the recoverable direction.
    fn deadline<I>(&self, index: &I, topic: &TopicId, partition: PartitionId) -> Option<i64>
    where
        I: MaterializedIndex + ?Sized,
    {
        let span = index.time_span(topic, partition)?;
        if span.max() == Timestamp::EPOCH {
            return None;
        }
        span.max().as_millis().checked_add(self.retention_ms)
    }
}
