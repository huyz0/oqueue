//! FR-35 under M10's deterministic simulation: a stale reader racing a
//! deleter (`M5.22`).
//!
//! ⚠️ **The invariant is about readers inside the bounds.** A reader that
//! resolves from an index at most `metadata_staleness` old and fetches within
//! `fetch_duration`, against a deleter whose clock leads by up to
//! `clock_skew`, must never see a 404. A reader *outside* them may — and must
//! then resolve it by refreshing: the refreshed index no longer names what it
//! asked for, which is the client's `OFFSET_OUT_OF_RANGE` reset.

#![allow(clippy::expect_used)]

use crate::support::{committed, key, topic};
use oqueue_compact::{
    DELETION_DELAY_MS, GcTerms, Lifecycle, MAX_CLOCK_SKEW_MS, MAX_IN_FLIGHT_FETCH_MS,
};
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, Error, FakeMaterializedIndex, FakeObjectStore,
    MAX_METADATA_STALENESS_MS, MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey,
    ObjectStore, Offset, PartitionId, Timestamp,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use tokio::time::{Instant, sleep};

/// A small deterministic generator, so a seed fixes every draw.
struct Draw(Cell<u64>);

impl Draw {
    fn below(&self, bound: i64) -> i64 {
        let next = self
            .0
            .get()
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0.set(next);
        let bound = u64::try_from(bound.max(1)).expect("positive");
        i64::try_from((next >> 33) % bound).expect("fits")
    }
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

fn millis(start: Instant) -> i64 {
    i64::try_from(start.elapsed().as_millis()).expect("a short run")
}

/// What one seeded run observed.
#[derive(Debug, Default)]
struct Run {
    in_bound_reads: usize,
    in_bound_404s: usize,
    late_404s: usize,
    unresolved_404s: usize,
    deleted: usize,
}

/// Every object a fetch from the log start resolves to right now.
fn resolve(index: &FakeMaterializedIndex) -> Vec<ObjectKey> {
    let start = index.log_start(&topic(), partition());
    index
        .find_batches(&topic(), partition(), start, u64::MAX)
        .expect("a fetch from the log start")
        .iter()
        .map(|batch| batch.reference().object().clone())
        .collect()
}

#[expect(clippy::too_many_lines, reason = "one scenario, read top to bottom")]
#[expect(
    clippy::future_not_send,
    reason = "run on run_seeded's current-thread runtime, where Rc is the honest share"
)]
async fn race(seed: u64) -> Run {
    let terms = GcTerms::CONFIGURED;
    let staleness = terms.metadata_staleness_ms;
    let draw = Rc::new(Draw(Cell::new(seed)));
    let index = Rc::new(FakeMaterializedIndex::new());
    let store = Rc::new(FakeObjectStore::new());
    let run = Rc::new(RefCell::new(Run::default()));
    let start = Instant::now();
    let skew = draw.below(MAX_CLOCK_SKEW_MS);
    let horizon = 4 * DELETION_DELAY_MS;

    // The writer and the deleter: an object a second, retention keeping the
    // newest ten, and a lifecycle sweep every second on a clock `skew` ahead.
    let writer = {
        let (index, store, run) = (Rc::clone(&index), Rc::clone(&store), Rc::clone(&run));
        async move {
            let mut lifecycle =
                Lifecycle::new(DELETION_DELAY_MS, terms).expect("the configured terms hold");
            let mut version = 0_u64;
            let mut written = 0_i64;
            while millis(start) < horizon {
                let name = key(&format!("obj-{written}"));
                store
                    .put(&name, vec![0_u8; 8], None)
                    .await
                    .expect("the fake accepts a put");
                version += 1;
                index
                    .apply(&[committed(
                        version,
                        name,
                        vec![CommittedSpan::new(
                            topic(),
                            partition(),
                            1,
                            ByteRange::Full,
                            None,
                        )],
                    )])
                    .expect("a commit");
                written += 1;
                if written > 10 {
                    let dropped = resolve(&index);
                    let trim_to = written - 10;
                    version += 1;
                    index
                        .apply(&[MetadataEntry::new(
                            CommitVersion::new(version),
                            MetadataRecord::Trimmed {
                                topic: topic(),
                                partition: partition(),
                                start: Offset::new(trim_to).expect("a valid offset"),
                            },
                        )])
                        .expect("a trim inside the log");
                    for object in dropped.into_iter().filter(|o| index.references(o) == 0) {
                        lifecycle.release(object);
                    }
                }
                let now = Timestamp::from_millis(millis(start) + skew).expect("a valid time");
                let report = lifecycle.sweep(&*index, &*store, now).await;
                run.borrow_mut().deleted += report.deleted;
                sleep(Duration::from_secs(1)).await;
            }
        }
    };

    // Readers: resolve, sit on the answer for a drawn time, then fetch. One in
    // eight overstays the bounds on purpose, to exercise the refresh path.
    let reader = |id: u64| {
        let (index, store, run, draw) = (
            Rc::clone(&index),
            Rc::clone(&store),
            Rc::clone(&run),
            Rc::clone(&draw),
        );
        async move {
            sleep(Duration::from_millis(id * 137)).await;
            while millis(start) < horizon {
                let resolved = resolve(&index);
                let late = draw.below(8) == 0;
                let hold = if late {
                    DELETION_DELAY_MS + draw.below(DELETION_DELAY_MS)
                } else {
                    draw.below(staleness) + draw.below(MAX_IN_FLIGHT_FETCH_MS)
                };
                sleep(Duration::from_millis(
                    u64::try_from(hold).expect("positive"),
                ))
                .await;
                for object in resolved {
                    let fetched = store.get(&object, ByteRange::Full).await;
                    let mut run = run.borrow_mut();
                    if !late {
                        run.in_bound_reads += 1;
                    }
                    if matches!(fetched, Err(Error::ObjectNotFound { .. })) {
                        if late {
                            run.late_404s += 1;
                        } else {
                            run.in_bound_404s += 1;
                        }
                        // 404 ⇒ refresh: a resolvable miss is one the
                        // refreshed index no longer names.
                        if resolve(&index).contains(&object) {
                            run.unresolved_404s += 1;
                        }
                    }
                }
            }
        }
    };

    tokio::join!(writer, reader(0), reader(1), reader(2), reader(3));
    Rc::try_unwrap(run)
        .expect("every task is done")
        .into_inner()
}

/// ⚠️ **FR-35's own verification method**: no reader inside the bounds ever
/// observes a 404, and every 404 a reader outside them observes is resolved by
/// refresh — across seeds, each of which fixes the schedule, the skew and
/// every hold.
#[test]
fn a_stale_reader_racing_a_deleter() {
    for seed in 0..8 {
        let run = oqueue_testkit::run_seeded(seed, || race(seed));
        assert!(run.deleted > 0, "seed {seed}: the deleter deleted, {run:?}");
        assert!(run.in_bound_reads > 0, "seed {seed}: readers read, {run:?}");
        assert!(
            run.late_404s > 0,
            "seed {seed}: a late reader met a deletion, {run:?}"
        );
        assert_eq!(run.in_bound_404s, 0, "seed {seed}: {run:?}");
        assert_eq!(run.unresolved_404s, 0, "seed {seed}: {run:?}");
    }
}

/// ⚠️ **Checked at startup, not assumed**: the configured delay passes, and a
/// delay at the bound — not above it — is refused, as is one the inequality's
/// terms would break if any grew.
#[test]
fn the_inequality_is_checked_when_the_lifecycle_starts() {
    let terms = GcTerms::CONFIGURED;
    assert!(Lifecycle::new(DELETION_DELAY_MS, terms).is_ok());
    assert_eq!(
        terms.bound(),
        i64::try_from(MAX_METADATA_STALENESS_MS).expect("fits")
            + MAX_IN_FLIGHT_FETCH_MS
            + MAX_CLOCK_SKEW_MS
    );
    assert!(matches!(
        Lifecycle::new(terms.bound(), terms),
        Err(Error::GcInequalityViolated { .. })
    ));
    assert!(Lifecycle::new(terms.bound() + 1, terms).is_ok());
    let slower = GcTerms {
        fetch_duration_ms: DELETION_DELAY_MS,
        ..terms
    };
    assert!(Lifecycle::new(DELETION_DELAY_MS, slower).is_err());
}
