//! `NFR-21` and its two neighbours, checked at every step of a run.
//!
//! ⚠️ **This is the row `M10.0` moved the requirement here for.**
//! `requirements.md` gives `NFR-21` the verification method "invariant test:
//! the cache is never ahead of durability", and M3 closed with the substance
//! asserted under FR-10's and FR-11's gate legs and nothing connecting the
//! requirement to evidence. This file is that evidence, and `M10.15` carries
//! the leg that runs it.
//!
//! ⚠️ **Durability is read from the store, and two earlier drafts read it from
//! the acknowledgement instead.** The first called a store-side reading
//! "circular", which was wrong — `broker.store` is a fixture this test owns
//! and can inspect independently of anything the broker says. The second kept
//! the acked reading and was *falsified by its own run*: an observation taken
//! while a produce was in flight saw records that were committed and durable
//! but not yet acknowledged, and reported a violation. ⚠️ **Durability is
//! established at the commit and the acknowledgement is delivered after it**,
//! so the two are not the same instant and only one of them is what `NFR-21`
//! is about.
//!
//! ⚠️ **The store reading rests on one fixture fact, asserted rather than
//! assumed**: this run's produces are one batch of two records each, and
//! `Cluster::flush` writes one object per flush (FR-32), so `n` objects means
//! offsets `0..2n` are durable. The run asserts the object count it expects,
//! so a change to either fact fails here rather than quietly rescaling the
//! invariant.
//!
//! ⚠️ **Checked after every step, including the failing ones.** A run that only
//! checked after successful produces would skip exactly the windows the
//! invariant is about — a pending PUT, a refused commit, a storm — which is
//! `M10.10`'s "across a run, not at its end" in the concrete.
//!
//! ⚠️ **And an observation the harness cannot take is skipped, not guessed.**
//! A refused fetch read as "end of the log" reports a violation against a
//! broker doing nothing wrong — measured by review. `observe` returns `None`
//! there, and the run asserts how many observations it genuinely took.
//!
//! ⚠️ **`overlap.rs` is a sibling, not a second concept** (`M10.32`,
//! `code-structure.md`'s 500-line limit). Two produces genuinely overlapping
//! one park is still this file's own claim, checked the same way.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`. `unreachable_pub`
// and `redundant_pub_crate` each refuse what the other asks for, and neither
// can tell a test binary's shared module from a library's.
#![allow(unreachable_pub)]
#![allow(clippy::redundant_pub_crate)]

use crate::roundtrip::{fetch_now, fetch_response_of, parked_fetch_frame, produce, produce_frame};
use crate::support::{Broker, broker};
use kafka_protocol::messages::produce_response::PartitionProduceResponse;
use oqueue_broker::{Dispatcher, Handler as _, HandlerResponse};
use oqueue_codec::error_codes::{LEADER_NOT_AVAILABLE, OFFSET_OUT_OF_RANGE};
use oqueue_core::{FaultConfig, LogFaults, StormKind};
use oqueue_testkit::{Invariants, Observation};
use std::sync::Arc;

/// The seed this run is described by, from `OQUEUE_SEED` or a fixed default.
///
/// ⚠️ **Read from the environment so the corpus and the sweep can drive it**
/// (`M10.11`) — a seed is an *input*, not a threshold, so non-negotiable 2 is
/// not in play: nothing passes or fails because of its value.
///
/// ⚠️ **And what it varies today is the schedule, not the sequence.** The steps
/// below are written down rather than generated, so a seed changes only what
/// `ADR-0028` seeds — `tokio`'s branch order — which is why the sweep is a
/// search over interleavings rather than over workloads. `M10.12` is where a
/// generated schedule would come from.
/// ⚠️ **A set-but-unparseable value panics rather than falling back.** An
/// earlier version wrote `.ok().and_then(..).unwrap_or(10)`, so
/// `OQUEUE_SEED=abc` — or a corpus row whose seed does not fit a `u64` — ran
/// the *default* schedule and reported the row as replayed. A green tick over
/// a schedule nobody visited is the failure this whole row exists to prevent.
pub fn seed() -> u64 {
    std::env::var("OQUEUE_SEED").map_or(10, |raw| {
        raw.parse().unwrap_or_else(|_| {
            panic!(
                "OQUEUE_SEED={raw} is not a u64, and silently running another \
                 schedule would report a replay that did not happen"
            )
        })
    })
}

/// How many fetches one observation will spend before calling it a loop.
///
/// ⚠️ **Bounded**, so a broker answering the same records forever fails this
/// file rather than hanging the suite inside `check-budget.sh`'s ceiling.
const FETCH_BUDGET: usize = 64;

/// What the world looks like to a client right now, or `None` if it could not
/// be looked at.
///
/// ⚠️ **A fetch answers from one object, so servability takes a loop.** The
/// first version read a single fetch and reported one batch as everything
/// servable, which failed the watermark invariant at step two against a broker
/// that was right — a *measurement* saying the system was wrong, which is the
/// worst thing an invariant harness can do.
///
/// ⚠️ **The watermark is taken from the first answer.** It is the number a
/// client acts on, and taking it from the last would compare an advertisement
/// made after the reads against reads made before it.
pub async fn observe(
    dispatcher: &Dispatcher,
    broker: &Broker,
    orphans: i64,
) -> Option<Observation> {
    // ⚠️ **Sampled before the reads, not after.** The comparison is
    // `visible <= durable`, so a durability figure taken *later* than the
    // offsets it is compared against is the lenient ordering: a write that was
    // pending when the first fetch went out has the whole loop to land in, and
    // the observation then pairs early visibility with late durability — which
    // is exactly the window this is here to catch, recorded as healthy.
    let durable_through = durable_through(broker, orphans);
    let mut visible_offsets: Vec<i64> = Vec::new();
    let mut high_watermark = None;
    for _ in 0..FETCH_BUDGET {
        let next = visible_offsets.last().map_or(0, |last| last + 1);
        let response = fetch_now(dispatcher, broker, "orders", next).await;
        let partition = &response.responses[0].partitions[0];
        // ⚠️ **Three outcomes, not two, and collapsing them loses the defect
        // this loop exists to find.** `OFFSET_OUT_OF_RANGE` means the reader
        // asked past the end — an ordinary terminator, and the answer a gapped
        // log gives once the gap has pushed the reader past the watermark. A
        // *refused* fetch is a store the harness cannot see through. Returning
        // `None` for both threw away the offsets already collected, so a
        // genuine gap died as "a run that checked fewer times than it acted",
        // pointing a reader at the harness rather than at the log.
        if partition.error_code == OFFSET_OUT_OF_RANGE {
            break;
        }
        if partition.error_code != 0 {
            return None;
        }
        high_watermark.get_or_insert(partition.high_watermark - 1);
        let got = partition.records.as_ref().map_or_else(Vec::new, offsets_in);
        if got.is_empty() {
            break;
        }
        visible_offsets.extend(got);
    }
    let servable_through = visible_offsets.last().copied().unwrap_or(-1);
    Some(Observation {
        visible_offsets,
        durable_through,
        // ⚠️ **Kafka's watermark is exclusive and this field is not**, so the
        // conversion happens here, once, beside the call that produced it.
        high_watermark: high_watermark.unwrap_or(-1),
        servable_through,
    })
}

/// Records per produce in this run, which is what turns objects into offsets.
const RECORDS_PER_FLUSH: i64 = 2;

/// The highest offset whose bytes are durable and carry assigned offsets.
///
/// ⚠️ **Read from the store rather than from the broker**, which is what makes
/// it evidence: the broker is the thing under test, and an invariant that
/// asked it whether it was right would be checking its opinion.
///
/// ⚠️ **Minus the orphans, and getting that wrong is a false negative in the
/// one invariant this row exists for.** An object whose commit was refused is
/// durable and carries *no assigned offsets*, so counting it inflates this by
/// a whole flush — and the error is in the lenient direction, so after the
/// refused-journal step a genuine `NFR-21` violation of two offsets would have
/// been reported green. The count of such objects is not read from the broker
/// either: `LEADER_NOT_AVAILABLE` is what the *client* was told, and it means
/// exactly "the bytes landed and the position did not".
fn durable_through(broker: &Broker, orphans: i64) -> Option<i64> {
    let objects = i64::try_from(broker.store.inner().len()).expect("a small store");
    let carrying = objects - orphans;
    (carrying > 0).then(|| carrying * RECORDS_PER_FLUSH - 1)
}

/// The offsets a fetch answered with, in the order served.
///
/// ⚠️ **The offsets, not a count.** A count-then-`(0..count)` reconstruction
/// makes "gap-free" true by fabrication — review proved it by making the
/// coordinator skip every other offset and watching the run stay green.
///
/// ⚠️ **Two decoders, each authoritative for its own part.** A `records` field
/// holds *concatenated* batches, so batch boundaries come from `oqueue_codec`'s
/// own framing and the records inside each from the dependency (`ADR-0017`).
fn offsets_in(records: &bytes::Bytes) -> Vec<i64> {
    use kafka_protocol::records::RecordBatchDecoder;
    let mut at = 0_usize;
    let mut offsets = Vec::new();
    while at < records.len() {
        let batch = &records[at..];
        let header = oqueue_codec::batch::decode_batch_header(batch).expect("a batch");
        // The length field excludes the base offset and itself: 8 + 4 bytes.
        let framed = usize::try_from(header.batch_length).expect("a small batch") + 12;
        let mut one = bytes::Bytes::copy_from_slice(&batch[..framed]);
        let decoded =
            RecordBatchDecoder::decode(&mut one).expect("the broker's own records decode");
        offsets.extend(decoded.records.iter().map(|record| record.offset));
        at += framed;
    }
    offsets
}

/// The run's own fixture property: this log starts empty, so a reader that
/// fetched from zero must be served from zero.
///
/// ⚠️ **Here rather than in the checker, because it is not an invariant** — a
/// nonzero first offset is legal in general (retention has run) and a defect
/// only *in this run*. ⚠️ **And gap-freeness does not cover it**: a stamp
/// shifted by one produces `[1,2,3,4]`, which has no gap — review's suggested
/// mutation, and without this it fails on an unrelated assertion or not at all.
pub fn starts_at_the_beginning(at: &Observation) {
    if let Some(&first) = at.visible_offsets.first() {
        assert_eq!(
            first, 0,
            "this run's log starts empty, so a fetch from zero is served from \
             zero — {first} means offsets were stamped somewhere they were not \
             assigned"
        );
    }
}

/// One run: the broker under test, the checker, and what the client has been
/// told so far.
///
/// ⚠️ **A struct because five separate `&mut` arguments made every call site
/// four lines after rustfmt**, which took the run past
/// `code-structure.md`'s fifty. The bundle is also the honest shape: `acked`
/// and `orphans` are facts *about this run* that only make sense beside the
/// checker they feed.
pub struct Run {
    pub dispatcher: Dispatcher,
    pub invariants: Invariants,
    /// The highest offset any produce was told had been accepted.
    pub acked: Option<i64>,
    /// Objects that landed and were never named by a commit.
    pub orphans: i64,
    /// How many times a parked fetch has actually raced a commit — one or
    /// two of them, `RacedProduce` and `overlap`'s step alike.
    ///
    /// ⚠️ **Exists for `generated.rs`'s gutting tests.** Either arm gutted to
    /// an ordinary produce is otherwise invisible from outside this file, and
    /// this is the one fact only the real path produces.
    pub races: u32,
}

impl Run {
    pub fn new(broker: &Broker) -> Self {
        Self {
            dispatcher: Dispatcher::new(Arc::clone(&broker.cluster)),
            invariants: Invariants::for_seed(seed()),
            acked: None,
            orphans: 0,
            races: 0,
        }
    }

    /// Produces one batch, then observes — and checks, if it could observe.
    pub async fn step(&mut self, broker: &Broker) {
        let response = produce(&self.dispatcher, broker, &["orders"]).await;
        self.absorb(&response.responses[0].partition_responses[0]);
        if let Some(at) = observe(&self.dispatcher, broker, self.orphans).await {
            starts_at_the_beginning(&at);
            self.invariants
                .check(&at)
                .unwrap_or_else(|broken| panic!("{broken}"));
        }
    }

    /// Folds one produce's answer into `acked`/`orphans` — split out so
    /// `overlap`'s two-produce race can fold both without duplicating what
    /// counts as accepted or orphaned. ⚠️ **`max`, not overwrite**: needed
    /// once a caller folds two, so the higher offset survives fold order.
    pub(crate) fn absorb(&mut self, partition: &PartitionProduceResponse) {
        if partition.error_code == 0 {
            let at = partition.base_offset + RECORDS_PER_FLUSH - 1;
            self.acked = Some(self.acked.map_or(at, |prev| prev.max(at)));
        }
        // ⚠️ **The client's own code, not the broker's internals.**
        // `LEADER_NOT_AVAILABLE` from a produce means the bytes landed and the
        // position did not, which is precisely an object carrying no offsets.
        if partition.error_code == LEADER_NOT_AVAILABLE {
            self.orphans += 1;
        }
    }
}

/// Storms the store past the observation's own GETs and requires the harness
/// to decline rather than to guess.
///
/// ⚠️ **This is the case review measured**: a refused fetch read as an empty
/// log reports a watermark violation against a broker doing nothing wrong,
/// which is the worst thing an invariant harness can do.
async fn a_refused_look_is_declined(run: &Run, broker: &Broker) {
    broker.store.inner().set_faults(FaultConfig {
        storm: Some((StormKind::Transient, 32)),
        ..FaultConfig::default()
    });
    let before = run.invariants.steps();
    assert!(
        observe(&run.dispatcher, broker, run.orphans)
            .await
            .is_none(),
        "a refused fetch cannot be observed, and guessing is how a harness \
         reports a violation the system did not have"
    );
    assert_eq!(
        run.invariants.steps(),
        before,
        "a declined look is not a check"
    );
    broker.store.inner().set_faults(FaultConfig::default());
}

/// ⚠️ **The run.** An in-flight look while the log is empty, then five
/// produces — healthy, stormed, healed, journal-refused, healthy — each
/// followed by an observation, plus one observation the harness declines to
/// take.
/// ⚠️ **`run_seeded`, not `#[tokio::test]`, and that is the whole of what
/// makes a seed mean anything.** An earlier version read `OQUEUE_SEED` and
/// handed it only to `Invariants::for_seed`, which uses it to *label* a
/// failure — so every seed executed the identical schedule, the nightly sweep
/// was thirty-two repetitions of one run, and any number it filed had no
/// causal relation to what failed. `ADR-0028` seeds `tokio`'s branch order
/// through `Builder::rng_seed`, and `oqueue_testkit::run_seeded` is the only
/// thing in this workspace that calls it.
#[test]
fn the_three_invariants_hold_at_every_step_of_a_faulted_run() {
    oqueue_testkit::run_seeded(seed(), || async {
        the_run().await;
    });
}

/// ⚠️ **`M10.31`'s whole point, proved directly**: `in_flight_is_not_visible`
/// entered *after* the log already holds a flush — see that function's own
/// doc for why `put_latency_polls` makes this reachable. `the_run` still
/// enters the window once, at the first produce, since retrofitting a second
/// entry changes that fixture's own cross-checked arithmetic; this stands on
/// its own instead.
#[test]
fn the_in_flight_window_is_visible_after_the_log_already_holds_data() {
    oqueue_testkit::run_seeded(seed(), || async {
        let broker = broker(&["orders"]).await;
        let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
        let mut invariants = Invariants::for_seed(seed());

        // Settle one flush first, so the log is not empty.
        let response = produce(&dispatcher, &broker, &["orders"]).await;
        assert_eq!(response.responses[0].partition_responses[0].error_code, 0);
        let settled = observe(&dispatcher, &broker, 0)
            .await
            .expect("an unfaulted fetch always observes");
        invariants.check(&settled).unwrap_or_else(|b| panic!("{b}"));

        // A second PUT in flight, observed mid-flight, over the first flush.
        in_flight_is_not_visible(&broker, &mut invariants, 0).await;
        assert_eq!(broker.store.inner().len(), 2, "both flushes landed");
    });
}

pub async fn the_run() {
    let broker = broker(&["orders"]).await;
    let mut run = Run::new(&broker);

    // ⚠️ **First, while the log is still empty**, which is the only moment the
    // in-flight window can be entered — see that function's own note.
    in_flight_is_not_visible(&broker, &mut run.invariants, 0).await;
    run.step(&broker).await;

    broker.store.inner().set_faults(FaultConfig {
        storm: Some((StormKind::Transient, 1)),
        ..FaultConfig::default()
    });
    run.step(&broker).await;
    broker.store.inner().set_faults(FaultConfig::default());
    run.step(&broker).await;

    broker.log.set_faults(LogFaults {
        refuse_append: true,
        ..LogFaults::default()
    });
    run.step(&broker).await;
    broker.log.set_faults(LogFaults::default());
    run.step(&broker).await;

    a_parked_fetch_races_a_commit(&broker, &mut run).await;
    a_refused_look_is_declined(&run, &broker).await;

    assert_eq!(
        run.invariants.steps(),
        7,
        "the in-flight look, five produces and the raced one — a run that checked fewer \
         times than it acted is the end-of-run check this row exists to replace"
    );
    assert_eq!(
        run.orphans, 1,
        "one produce wrote its object and lost its commit"
    );
    assert_eq!(
        run.acked,
        Some(9),
        "five flushes were acknowledged — the in-flight one, three of the five \
         steps, and the one the parked fetch raced — at two records each"
    );
    // ⚠️ **The fixture fact the durability reading rests on.** Five objects:
    // the four acknowledged flushes — the in-flight one and three steps — plus
    // the one whose journal refused, which wrote its bytes and then could not
    // name them. ⚠️ **The stormed flush leaves none**, which is the difference
    // between the two failing steps and the reason both are in the run.
    assert_eq!(
        broker.store.inner().len(),
        6,
        "one object per flush, and a refused commit still leaves its object"
    );
}

/// Observes while a PUT is still in flight — at any point in a run, not just
/// while the log is empty. This is the window `NFR-21` names.
///
/// ⚠️ **`put_latency_polls`, not `latency_polls`** (`M10.31`, from `M10.10`,
/// which measured the gap). `latency_polls` delays every store call, so an
/// in-flight look's own GETs get delayed too, and the fake's self-waking
/// `Pending` lets the produce land during those fetches. An empty log needed
/// **no** GET (FR-12) and escaped this, which is why `M10.10` could only
/// enter the window once, at the first produce. `put_latency_polls` delays
/// only the `put`, so the observation's GETs run at ordinary speed regardless.
async fn in_flight_is_not_visible(broker: &Broker, invariants: &mut Invariants, orphans: i64) {
    broker.store.inner().set_faults(FaultConfig {
        put_latency_polls: 200,
        ..FaultConfig::default()
    });
    let before = broker.store.inner().len();
    let frame = produce_frame(broker, "orders").await;
    let writing = {
        let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
        tokio::spawn(async move { dispatcher.handle(frame).await })
    };
    // Let the produce start and reach its PUT.
    for _ in 0..2 {
        tokio::task::yield_now().await;
    }
    // ⚠️ A length delta, not `is_empty()` — `M10.10`'s own check assumed one.
    assert_eq!(
        broker.store.inner().len(),
        before,
        "the PUT has landed already, so this look is not in flight"
    );
    let looking = Dispatcher::new(Arc::clone(&broker.cluster));
    let at = observe(&looking, broker, orphans)
        .await
        .expect("the fetch is not what was slowed");
    invariants
        .check(&at)
        .unwrap_or_else(|broken| panic!("in flight: {broken}"));

    broker.store.inner().set_faults(FaultConfig::default());
    let HandlerResponse::Reply(_) = writing.await.expect("the produce task lives") else {
        panic!("the produce answers");
    };
}

/// How long a parked fetch is willing to wait, in virtual milliseconds.
///
/// ⚠️ **Short enough that a lost wakeup is a failure rather than a hang.**
/// Under `run_seeded`'s paused clock this costs no real time.
pub(crate) const PARK_MS: i32 = 50;

/// A fetch parked at the high watermark while a produce commits underneath it.
///
/// ⚠️ **The only place in the written run where a `select!` is even
/// reached** — `fetch_now`'s `max_wait_ms = 0` returns before `park.rs`'s,
/// and `session.rs`'s and `connection.rs`'s are off the `Dispatcher` path
/// entirely. Without this step the seed decides nothing and the sweep is
/// repetitions of one schedule.
///
/// ⚠️ **Not a genuine race, correcting this doc's own prior claim**
/// (`M10.32`, measured directly). One serialized produce always commits
/// before the deadline can elapse, so the commit wins every time and the
/// timeout branch below is dead code this step can never reach.
/// `overlap::two_produces_race_one_parked_fetch` is where two produces
/// genuinely overlap a park instead of one serializing under it.
pub async fn a_parked_fetch_races_a_commit(broker: &Broker, run: &mut Run) {
    run.races += 1;
    let at = i64::from(u32::try_from(run.acked.map_or(0, |last| last + 1)).unwrap_or(0));
    let frame = parked_fetch_frame(broker, "orders", at, PARK_MS).await;
    let parked = {
        let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
        tokio::spawn(async move { dispatcher.handle(frame).await })
    };
    run.step(broker).await;

    let HandlerResponse::Reply(reply) = parked.await.expect("the parked task lives") else {
        panic!("a fetch answers");
    };
    let response = fetch_response_of(&reply);
    let partition = &response.responses[0].partitions[0];
    assert_eq!(
        partition.error_code, 0,
        "a parked fetch that loses its race answers empty, never an error"
    );
}
