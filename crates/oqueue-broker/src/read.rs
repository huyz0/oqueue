//! The read path: index lookup, object reads, and the offset stamp.
//!
//! ⚠️ **Its own module because a fetch is where the index's two tiers stop
//! being symmetric.** A tail read is one GET at a range the index already
//! holds; a history read is a footer resolution first, because the index
//! deliberately does not keep byte ranges for entries past its tail window —
//! `ADR-0022`, and the ~40-bytes-per-entry arithmetic in `M3.md` is why.
//!
//! ⚠️ **A history read GETs the whole object, and that is still a bound rather
//! than a choice.** Resolving a footer from a suffix needs the object's size,
//! and [`ObjectStore`](oqueue_core::ObjectStore) has no `head` — three
//! methods, `get`/`put`/`delete`. Adding one is a seam change with an ADR and
//! every implementation behind it. ⚠️ **`M3.22` did not close this**, and said
//! so: it bounded how *many* objects a read touches, not how much of each one
//! it fetches. The ranged read is `M5`'s, alongside the compaction that
//! rewrites these objects.
//!
//! ⚠️ **The byte budget and the 404 rule are here** (`M3.22`). The budget is
//! spent as each object's size is *learned*, because `find_batches` can only
//! price what has a byte range and a history entry has none (`ADR-0022`). The
//! 404 rule is [`read_or_refresh`](Cluster::read_or_refresh): an object the
//! index named and the store does not have was **reaped**, never "not yet
//! written", so it is out of range rather than end-of-log. ⚠️ **The refresh
//! that name promises is `M7`'s**, and the function says why it is a
//! passthrough in a broker whose index is the coordinator's own.

mod batch;
mod manifest;

use crate::cluster::Cluster;
use crate::fetch::Allowance;
use oqueue_codec::batch::rewrite_base_offset;
use oqueue_core::{ByteRange, Error, ObjectKey, Offset, PartitionId, TopicId};
use std::collections::HashMap;
use tracing::Instrument;

impl Cluster {
    /// Every batch this partition holds from `start`, with its real base
    /// offset stamped in.
    ///
    /// ⚠️ **A fetch *at* the high watermark issues zero GETs** (FR-12): the
    /// index names no batch at or past the end offset, so the loop below never
    /// runs and the store is never touched.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`] if a stored entry's end offset is
    /// unrepresentable; [`Error::IndexObjectMismatch`] if an object does not
    /// hold what the index said it does; [`Error::ObjectNotFound`] if an object
    /// the index named is gone — see [`Cluster::read_or_refresh`] for what that
    /// means; and whatever else the store returns.
    pub async fn read(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        spend: &mut Spend<'_>,
    ) -> Result<Read, ReadFailure> {
        let span = crate::telemetry::operation_span("fetch", "fetch");
        let result = self
            .read_inner(topic, partition, start, spend)
            .instrument(span.clone())
            .await;
        span.record(
            "outcome",
            if result.is_ok() { "success" } else { "failure" },
        );
        result
    }

    async fn read_inner(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        spend: &mut Spend<'_>,
    ) -> Result<Read, ReadFailure> {
        let allowance = spend.allowance;
        // ⚠️ **The manifest tier comes first, and the index cannot name it.**
        // Below the offset a published manifest covers, `find_batches` returns
        // nothing — the entries it would have returned are what the manifest
        // replaced (`ADR-0042`) — so a fetch there is answered by reading the
        // manifest object. It costs one GET and buys *ranged* reads for a tier
        // that otherwise costs a whole object each to resolve a footer.
        let mut fetched: u64 = 0;
        // ⚠️ **A partition with nothing to spend touches the store not at
        // all**, which the loop below achieves by breaking before its first
        // fetch — so resolving a manifest above it would be the one GET a
        // spent budget could not stop. `M5.63`'s first round.
        let can_fetch = allowance.bytes != 0 || allowance.may_overshoot;
        let mut page = match self.index().manifest(topic, partition) {
            Some((key, upto)) if start < upto && can_fetch => {
                let resolved = self.manifest_batches(&key, start, upto, spend).await;
                fetched += resolved.cost;
                resolved
                    .batches
                    .map_err(|error| ReadFailure { error, fetched })?
            }
            _ => Vec::new(),
        };
        // ⚠️ **Both tiers, not one or the other.** A fetch starting inside the
        // manifest and running past it must cross into what the index still
        // names, or a consumer reading forward stops at the boundary and
        // re-fetches the same offset forever. `find_batches` answers from
        // `start` and names nothing below `upto`, so the two lists abut.
        page.extend(
            self.index()
                .find_batches(topic, partition, start, allowance.bytes)
                .map_err(|error| ReadFailure { error, fetched })?,
        );
        let mut records: Vec<u8> = Vec::new();
        for batch in page {
            // ⚠️ **The budget is spent as it is learned, not priced up front.**
            // `find_batches` bounds what it can *price*, and a history entry
            // has no byte range to price at all — `ADR-0022` says so — so a
            // page can name up to `MAX_BATCHES_PER_PAGE` batches of unknown
            // size. Stopping here, once a batch's real size is known, is the
            // only place the client's number can actually bind.
            //
            // ⚠️ **One batch over the line is allowed, once per *response*.** A
            // partition whose very first batch is larger than the budget must
            // still be readable, or the consumer parks at that offset forever
            // re-fetching nothing — Kafka's own broker makes the same
            // exception. ⚠️ **But `may_overshoot` is the *response*'s answer,
            // not this partition's**: per partition, a client naming the same
            // partition two hundred times in one frame would get two hundred
            // whole batches for a `max_bytes` of one, and the budget would not
            // be a bound at all.
            //
            // ⚠️ **Measured against what was *fetched*, not what was
            // returned**, and the two are the same number only in the layout a
            // single-topic test happens to produce. A history batch is a
            // region of a bundle covering every partition that flush wrote, so
            // a page of sixty-four entries can pull sixty-four whole objects
            // off the store while the slices returned still add up to less
            // than a small `max_bytes` — the read never stops, and the charge
            // the caller applies afterwards is too late to have stopped it.
            // The `max` is what makes one expression cover both tiers: a tail
            // read's range *is* its batch on the success path, so `fetched`
            // and `records.len()` agree there — but `one_batch` still reports
            // a real, separate fetch cost on the *failure* path (`M3.37`),
            // which is what stops a stamp failure being charged nothing.
            let over = fetched.max(records.len() as u64) >= allowance.bytes;
            if over && !(records.is_empty() && allowance.may_overshoot) {
                break;
            }
            // ⚠️ **A miss after the first batch ends the read, it does not
            // undo it.** Records already in hand are at offsets that *are* in
            // range; discarding them to answer `OFFSET_OUT_OF_RANGE` would
            // tell a consumer that an offset it can be served is gone, and a
            // client resetting to `latest` would skip the very records this
            // broker had just read. Stopping here answers what is readable and
            // leaves the missing object to be the *first* batch of the next
            // fetch — where it is the honest out-of-range answer.
            let fetch = self.one_batch(&batch, topic, partition, spend).await;
            fetched += fetch.cost;
            let mut blob = match fetch.blob {
                Ok(blob) => blob,
                Err(_) if !records.is_empty() => break,
                // ⚠️ **The bytes go back with the error.** A read that pulled
                // a whole bundle off the store and then could not parse it
                // cost the request exactly what a successful one would have,
                // and dropping `fetched` here is how a frame naming K
                // partitions in K malformed bundles bought K whole-object GETs
                // against a `max_bytes` of one — charged nothing, because the
                // caller only ever saw an `Error`.
                Err(error) => return Err(ReadFailure { error, fetched }),
            };
            // ⚠️ The stamp `cluster.rs`'s module doc explains: bytes in storage
            // carry whatever base offset the producer sent, because they were
            // written before anyone knew what order they landed in. A blob too
            // short to hold a batch header is an object disagreeing with the
            // index that named it, so the read fails rather than serving it.
            if rewrite_base_offset(&mut blob, batch.reference().base_offset().get(), 0).is_err() {
                // ⚠️ **A parse failure, counted here and only on a miss.**
                // ⚠️ **Both tiers reach this**, and the comment said "a tail
                // read's" until `M3.37`: a history batch whose region slices
                // cleanly and is then too short to hold a batch header fails
                // the stamp too. `one_batch` counts the *slice* failure; this
                // counts the stamp's, for either tier.
                if fetch.missed {
                    spend.objects.note_failure();
                }
                return Err(ReadFailure {
                    error: Error::IndexObjectMismatch,
                    fetched,
                });
            }
            records.append(&mut blob);
        }
        Ok(Read { records, fetched })
    }

    /// [`read`](Self::read), with the 404 rule around it.
    ///
    /// ⚠️ **A 404 is never "end of log"** — hazard H4, and doc 12 §4.6 is
    /// explicit. Object ids are never reused, so an object the index named and
    /// the store does not have was **reaped**, not "not yet written": the
    /// index is behind a deletion, and the offsets it is still pointing at are
    /// gone for good. A reader that treated the miss as the end would tell a
    /// consumer it was caught up while records it had not read were being
    /// deleted underneath it — a successful poll returning nothing, which is
    /// the silent wrongness this milestone is written against. The caller
    /// answers `OFFSET_OUT_OF_RANGE`; it is counted here.
    ///
    /// ⚠️ **The row asks for a refresh and a retry, and in *this* broker there
    /// is nothing to refresh.** The index a fetch reads is the coordinator's
    /// own — the same object, in the same process, folded before the ack — and
    /// nothing in `M3` ever removes an entry from it. So a second read would
    /// consult provably identical state and answer identically — not because
    /// it would cost a second GET (`FetchedObjects` remembers a failure for
    /// the life of the request, so a retry against the same object is free),
    /// but because there is nothing a round trip to *this* index could learn
    /// that the first read did not already know. There is no retry here
    /// rather than a retry that re-reads a cache and calls it a round trip.
    ///
    /// ⚠️ **`M7` is where the refresh becomes real work**: a follower's index
    /// *is* a cache, its 404 may mean "my index is behind" rather than
    /// "reaped", and `IndexWatch` — the mechanism that actually waits for the
    /// coordinator to fold — is what a refresh would use. `roadmap.md` carries
    /// it.
    ///
    /// # Errors
    ///
    /// As [`read`](Self::read).
    pub async fn read_or_refresh(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        spend: &mut Spend<'_>,
    ) -> Result<Read, ReadFailure> {
        self.read(topic, partition, start, spend).await
    }
}

/// Every object one request has already pulled from the store.
///
/// ⚠️ **The bound on a request's reads, and the only one that can be.** A
/// `Fetch` names as many partitions as its frame holds, and nothing dedups
/// them: the same partition two hundred times is a legal request, and so is
/// four partitions that all live in one bundle. Charging *bytes* cannot bound
/// the GETs those cost — the client picks the byte numbers — so what bounds
/// them is not fetching the same thing twice.
///
/// ⚠️ **Failures are remembered too.** A store that refused an object once
/// will refuse it again, and a client repeating the entry that failed is
/// exactly the shape that turns one frame into a GET per entry.
///
/// ⚠️ **What it holds is bounded by the byte budget *per pass*, and only
/// because the read stops on bytes fetched.** Every successful GET is charged,
/// and a read stops as soon as what it has pulled crosses its allowance — so a
/// partition overshoots by at most the one object that crossed the line, and
/// once a pass's budget is spent its later partitions get a zero allowance and
/// never reach the store. ⚠️ **That is a claim about [`Cluster::read`]'s stop
/// condition**, not about this map: the map keeps every object it fetched, and
/// if that stop is ever weakened to count *returned* bytes again this grows
/// with the page rather than with the budget.
///
/// ⚠️ **Multiply by the passes.** Since `M3.26` this outlives one pass and
/// lives as long as the request, so a parked fetch holds up to
/// `MAX_READS_PER_REQUEST` budgets' worth plus the over-the-line objects — ⚠️
/// **more than one per pass since `M3.39`**, because a partition that returned
/// nothing no longer takes the exemption with it, so a second failing
/// partition can fetch a whole object past a spent budget before the failure
/// cap stops it. Bounded at `MAX_FAILED_FETCHES_PER_REQUEST` extra objects per
/// request, not one per pass — the price of not re-downloading a bundle once per wakeup, and the
/// number to size a broker's memory from. It was a per-object allocation
/// before `M3.22`, a per-pass one after it, and is a per-request one now.
///
/// ⚠️ **Failures are bounded separately** — see
/// [`MAX_FAILED_FETCHES_PER_REQUEST`]. A failed read is charged what it
/// *fetched*, same as `ReadFailure` below — usually nothing, because a
/// store refusal pulls nothing, but a GET that succeeded and then could not
/// be used (a parse or stamp failure past the store) spends exactly what a
/// successful read would. What a byte budget cannot bound is the *count*: a
/// request naming many distinct objects the store refuses outright can drive
/// an unbounded number of near-zero-byte GETs, which is what this cap stops.
#[derive(Debug, Default)]
pub struct FetchedObjects {
    seen: HashMap<(ObjectKey, RangeKey), Result<Vec<u8>, Error>>,
    failed: u32,
}

/// One batch's bytes, what fetching them cost, and whether a GET was issued.
///
/// ⚠️ **The third field travels with the other two because the failure cap
/// counts *distinct objects*.** A caller that learns a read failed but not
/// whether it missed cannot tell a store that refused an object from a client
/// naming the same bad object twice — and counting the second is how one
/// corrupt bundle refuses the healthy partitions behind it.
struct Fetched {
    blob: Result<Vec<u8>, Error>,
    pub(super) cost: u64,
    missed: bool,
}

/// A read that failed, and what it had already cost when it did.
///
/// ⚠️ **The pair, because a failure is not free.** A read that pulled a whole
/// bundle and then could not parse it spent exactly what a successful read
/// would have; an `Error` alone leaves the caller nothing to charge, which is
/// how a request naming many malformed objects escaped the budget entirely.
#[derive(Debug)]
pub struct ReadFailure {
    /// Why the read failed.
    pub error: Error,
    /// Bytes this read had already pulled from object storage.
    pub fetched: u64,
}

/// How many of one request's object reads may *fail* before it stops asking.
///
/// ⚠️ **Failures need their own bound because a store refusal need not cost
/// bytes.** A healthy read is bounded by the byte budget — it fetched
/// something, so it is charged for it. A read the store refuses outright
/// pulls nothing and is charged nothing — deliberately, so that one fault
/// cannot blank the healthy partitions behind it — and the object cache only
/// stops the *same* object being asked for twice. A frame naming sixty-four
/// partitions in sixty-four different bundles therefore bought sixty-four
/// GETs against a store that was failing all of them — the broker reading
/// *hardest* exactly when the store is least able to serve it, and rising
/// with the fan-out of the client's own subscription. ⚠️ **Not every
/// failure is this cheap** — a read that pulled a whole bundle and then
/// could not use it spends exactly what a successful one would, see
/// `ReadFailure` — but this cap bounds the *count* of failures either way,
/// which a byte budget alone cannot.
///
/// ⚠️ **Two, because the second is the one that carries information.** The
/// first failure could be a blip; a second distinct object failing in the same
/// frame says the store is unwell, and asking it sixty-two more times in that
/// frame learns nothing the retry on the client's next poll would not.
pub const MAX_FAILED_FETCHES_PER_REQUEST: u32 = 2;

/// A [`ByteRange`] in a form that can key a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RangeKey {
    Whole,
    Bounded { offset: u64, length: u64 },
}

impl From<ByteRange> for RangeKey {
    fn from(range: ByteRange) -> Self {
        match range {
            ByteRange::Full => Self::Whole,
            ByteRange::Bounded(bounded) => Self::Bounded {
                offset: bounded.offset(),
                length: bounded.length(),
            },
        }
    }
}

/// What a read may spend, and what the request has already fetched.
///
/// ⚠️ **One value because they are one question.** How many bytes this
/// partition may return, whether it is the read allowed to cross the line by a
/// batch, and which objects are already in hand are all "what may this read
/// cost the request" — and passing them apart is how the per-partition
/// exemption and the per-partition read count each got past review once.
#[derive(Debug)]
pub struct Spend<'a> {
    /// The bytes and the overshoot this partition is allowed.
    pub allowance: Allowance,
    /// What the request has already pulled from the store.
    pub objects: &'a mut FetchedObjects,
}

/// What one read produced, and what it cost.
///
/// ⚠️ **The two are not the same number.** A history batch lives inside a
/// bundle holding every partition one flush covered, so the records returned
/// can be a hundredth of the bytes pulled off the store — and a budget that
/// counted only what it returned would let a read walk sixty-four whole
/// bundles to answer with a megabyte.
#[derive(Debug)]
pub struct Read {
    /// The record batches, stamped with their assigned offsets.
    pub records: Vec<u8>,
    /// Bytes this read pulled from object storage that the request had not
    /// already pulled. ⚠️ **A cache hit costs nothing** — it issued no GET, so
    /// charging for it would let a client's own repetition exhaust its budget.
    pub fetched: u64,
}

impl FetchedObjects {
    /// Records a failure the store did not report: an object that arrived and
    /// then would not parse.
    ///
    /// ⚠️ **Same counter, and the caller must have missed.** The cap counts
    /// distinct objects the store could not usefully answer for, so a partition
    /// that took a bad object out of the cache issued no GET and adds nothing —
    /// `one_batch` checks that before calling this.
    pub(crate) const fn note_failure(&mut self) {
        self.failed += 1;
    }

    /// The object at `key` over `range`, fetched at most once per request.
    ///
    /// Returns the bytes and whether this call actually issued the GET — the
    /// caller charges its budget for a miss and nothing for a hit.
    async fn get(
        &mut self,
        cluster: &Cluster,
        key: &ObjectKey,
        range: ByteRange,
    ) -> (Result<Vec<u8>, Error>, bool) {
        let id = (key.clone(), RangeKey::from(range));
        if let Some(seen) = self.seen.get(&id) {
            return (seen.clone(), false);
        }
        // ⚠️ **Refused rather than attempted, and refused *loudly*.** The
        // caller turns any non-404 failure into `OFFSET_NOT_AVAILABLE`, which
        // is retried; answering an empty partition instead would be doc 12
        // §4.6's silent wrongness, and answering it for free is the point —
        // this arm issues no GET, so the store sees nothing.
        if self.failed >= MAX_FAILED_FETCHES_PER_REQUEST {
            return (Err(Error::SlowDown), false);
        }
        let get_span = crate::telemetry::dependency_span("object_store", "get", "fetch");
        let fetched = cluster
            .store()
            .get(key, range)
            .instrument(get_span.clone())
            .await;
        get_span.record(
            "outcome",
            if fetched.is_ok() {
                "success"
            } else {
                "failure"
            },
        );
        if fetched.is_err() {
            self.failed += 1;
        }
        // ⚠️ **Counted here, on the miss**, so the number is *distinct reaped
        // objects* rather than read attempts. A client naming one reaped
        // partition two hundred times must not be able to drive the operator
        // alarm to two hundred — an alarm a client chooses the value of is not
        // a signal anyone can act on.
        if matches!(fetched, Err(Error::ObjectNotFound { .. })) {
            cluster.count_reaped_read();
        }
        self.seen.insert(id, fetched.clone());
        (fetched, true)
    }
}
