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
//! ⚠️ **And an observation the harness cannot take is skipped, not guessed.** A
//! fetch the store refuses comes back with an error code and no records, which
//! read as "end of the log" reports a watermark violation against a broker
//! doing nothing wrong — measured by review. `observe` returns `None` there,
//! and the run asserts how many observations were genuinely taken so a harness
//! that silently stopped checking would fail.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`. `unreachable_pub`
// and `redundant_pub_crate` each refuse what the other asks for, and neither
// can tell a test binary's shared module from a library's.
#![allow(unreachable_pub)]
#![allow(clippy::redundant_pub_crate)]

use crate::roundtrip::{fetch_now, fetch_response_of, parked_fetch_frame, produce, produce_frame};
use crate::support::{Broker, broker};
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
/// ⚠️ **Bounded, so a broker that answered the same records forever fails this
/// file rather than hanging the suite** inside `check-budget.sh`'s ceiling.
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
/// ⚠️ **The offsets, not a count, and the first version returned a count.**
/// `visible_offsets` was then built as `(0..count)`, which makes "gap-free"
/// true by fabrication — review proved it by making the coordinator skip every
/// other offset and watching the run stay green. The checker's own field doc
/// says exactly this ("a count cannot tell 0,1,2 from 0,1,3") and the driver
/// was doing the thing it warns about.
///
/// ⚠️ **Two decoders, each authoritative for its own part.** A `records` field
/// holds *concatenated* batches and `RecordBatchDecoder` decodes one — handed
/// the whole field it reports "not enough bytes". So batch boundaries come
/// from `oqueue_codec`'s framing, this repository's own subject, and the
/// records inside each batch from the dependency (`ADR-0017`).
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
/// ⚠️ **Here rather than in the checker, because it is not an invariant.** A
/// log whose first readable offset is not zero is perfectly legal in general —
/// retention has run — and is a defect *in this run*, which starts from
/// nothing. ⚠️ **And gap-freeness does not cover it**: a stamp that shifted
/// every batch by one produces `[1,2,3,4]`, which has no gap. Review's
/// suggested mutation is exactly that shape, and without this it fails on an
/// unrelated assertion or not at all.
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
    /// How many times a parked fetch has actually raced a commit.
    ///
    /// ⚠️ **Exists for `generated.rs`'s `RacedProduce` test.** A `RacedProduce`
    /// arm gutted to an ordinary produce is otherwise invisible from outside
    /// this file — both outcomes are legal, so no assertion here can tell them
    /// apart — and this is the one fact only the real path produces.
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
        let partition = &response.responses[0].partition_responses[0];
        if partition.error_code == 0 {
            self.acked = Some(partition.base_offset + RECORDS_PER_FLUSH - 1);
        }
        // ⚠️ **The client's own code, not the broker's internals.**
        // `LEADER_NOT_AVAILABLE` from a produce means the bytes landed and the
        // position did not, which is precisely an object carrying no offsets.
        if partition.error_code == LEADER_NOT_AVAILABLE {
            self.orphans += 1;
        }
        if let Some(at) = observe(&self.dispatcher, broker, self.orphans).await {
            starts_at_the_beginning(&at);
            self.invariants
                .check(&at)
                .unwrap_or_else(|broken| panic!("{broken}"));
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

pub async fn the_run() {
    let broker = broker(&["orders"]).await;
    let mut run = Run::new(&broker);

    // ⚠️ **First, while the log is still empty**, which is the only moment the
    // in-flight window can be entered — see that function's own note.
    in_flight_is_not_visible(&broker, &mut run.invariants).await;
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

/// Observes while a PUT is still in flight.
///
/// ⚠️ **This is the window `NFR-21` names**, and no other observation in this
/// run is in it: `step` awaits its produce to completion, and the produce path
/// flushes inline, so every other look is at a settled state.
///
/// ⚠️ **It only works while the log is empty, and review measured why.**
/// `FaultConfig::latency_polls` delays *every* store call, so a look taken
/// while a PUT pends has its own GETs delayed too — and since the fake wakes
/// itself on each `Pending`, the produce lands during the observation's own
/// fetches and the "in-flight" look reports a settled state. Against an empty
/// log the observation needs **no** GET (FR-12: a fetch at the high watermark
/// issues none), so it is not delayed and the window is real. ⚠️ **That is a
/// limit of the knob, not a property of the system**: a PUT-only pause would
/// let this be checked mid-run as well, and `M10.31` is the row for it.
///
/// So a broker serving from an in-memory buffer before its bytes were durable
/// fails here and nowhere else — at the first produce only.
async fn in_flight_is_not_visible(broker: &Broker, invariants: &mut Invariants) {
    broker.store.inner().set_faults(FaultConfig {
        latency_polls: 200,
        ..FaultConfig::default()
    });
    let frame = produce_frame(broker, "orders");
    let writing = {
        let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
        tokio::spawn(async move { dispatcher.handle(frame).await })
    };
    // Let the produce start and reach its PUT.
    for _ in 0..2 {
        tokio::task::yield_now().await;
    }
    // ⚠️ **Asserted, because "in flight" is the whole claim and two earlier
    // versions of this were not.** With `latency_polls: 8` and four yields the
    // write had already landed by the time the look was taken — measured — so
    // the observation was of a settled state wearing the name of a window.
    assert!(
        broker.store.inner().is_empty(),
        "the PUT has landed already, so this look is not in flight and the \
         window NFR-21 names is not being entered"
    );
    let looking = Dispatcher::new(Arc::clone(&broker.cluster));
    let at = observe(&looking, broker, 0)
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
/// ⚠️ **Long enough that the deadline is not a foregone conclusion and short
/// enough that a lost wakeup is a failure rather than a hang.** Under
/// `run_seeded`'s paused clock this costs no real time, so the number is about
/// which branch of `park.rs`'s `select!` can win, not about duration.
const PARK_MS: i32 = 50;

/// A fetch parked at the high watermark while a produce commits underneath it.
///
/// ⚠️ **This is the only place in the run where the seed decides anything**,
/// and without it the whole row is machinery over a constant. Review measured
/// the previous state: the run made **zero** draws from `tokio`'s seeded RNG,
/// because `fetch_now` sends `max_wait_ms = 0` and `park.rs` returns on its own
/// deadline check before reaching the `select!`, `session.rs` returns before
/// its own, and `connection.rs`'s is not on the `Dispatcher` path. So every
/// seed executed one identical schedule and the sweep was thirty-two
/// repetitions of it.
///
/// ⚠️ **Both outcomes are legal and neither is asserted.** The parked fetch may
/// see the commit or may time out with nothing — that is exactly the choice
/// `select!` makes, and pinning either would be pinning one seed. What is
/// asserted is that the invariants hold whichever way it goes, which is what a
/// seeded harness is for.
pub async fn a_parked_fetch_races_a_commit(broker: &Broker, run: &mut Run) {
    run.races += 1;
    let at = i64::from(u32::try_from(run.acked.map_or(0, |last| last + 1)).unwrap_or(0));
    let frame = parked_fetch_frame(broker, "orders", at, PARK_MS);
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
