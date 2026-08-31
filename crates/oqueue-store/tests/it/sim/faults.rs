//! What the model can be told to do wrong, and the runtime a fault run needs.
//!
//! ⚠️ **Three faults, each chosen because M3 already ships the code path it
//! exercises** — `M10.7`. A fault with no production reader is a feature of
//! the harness rather than a test of the system:
//!
//!   - **A 503 storm** meets `retry.rs`'s translation and `object_store`'s
//!     retry loop, and is the only thing that can show where the shipped
//!     budget actually ends.
//!   - **A lost conditional-write race** meets `classify.rs`'s
//!     `Precondition`/`AlreadyExists` arm, which is the primitive `ADR-0005`
//!     puts under every commit.
//!   - **A refused delete** meets `s3.rs`'s per-key loop, and is the only way
//!     to observe that `delete` is not atomic across the keys it is given.
//!
//! ⚠️ **Armed, not configured.** A fault is pushed onto a plan and consumed by
//! the request that meets it, so a test says *what goes wrong and when* rather
//! than setting a mode the rest of the run then has to remember to clear.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]
// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the bare
// `pub` clippy's `redundant_pub_crate` asks for — the trade `model.rs` and
// `latency.rs` both make beside it, for the same reason.
#![allow(clippy::redundant_pub_crate)]

/// What S3 answers when it is shedding load, and what `object_store` retries.
///
/// ⚠️ **503 and not 500**, because the two are not the same test: `object_store`
/// retries both, but 503 is the code a real storm arrives as —
/// `docs/researches/04-object-storage-s3-gcs.md` §3 on a burst outrunning S3's
/// gradual repartitioning, and §2 on the same against a cold prefix, both
/// naming `503 Slow Down` specifically.
const SERVICE_UNAVAILABLE: u16 = 503;

/// One injected failure, armed before the run that meets it.
///
/// ⚠️ **`Debug` is hand-written and prints no payload**, which is
/// `ModelState`'s own rule ("counts, not contents") and applies here for the
/// same reason: a competitor writing a realistic segment would otherwise put a
/// megabyte of byte literals into a panic message. The length is what a reader
/// of a failure needs.
pub(crate) enum Fault {
    /// The next `remaining` requests with this method answer 503.
    ///
    /// ⚠️ **Counted in *requests*, which is what makes the retry budget
    /// visible.** `object_store` retries inside one `ObjectStore` call, so a
    /// storm of two is absorbed by a policy of three attempts and a storm of
    /// three is not — the boundary `retry_config_for` translates and nothing
    /// until now could reach.
    Storm {
        /// Matched against the request's method verbatim, so a storm aimed at
        /// writes does not consume a GET the same test happens to make.
        method: &'static str,
        /// Decremented per request answered, so a storm is spent rather than
        /// standing.
        remaining: u32,
    },
    /// A competitor lands `bytes` under `key` just before the next PUT to it
    /// is evaluated.
    ///
    /// ⚠️ **Injected *inside* the window, which is the whole point.** A test
    /// can already write a key twice and watch the second write lose; what it
    /// cannot do from outside is lose a race it was winning when it looked.
    /// This puts the competing write between the caller's decision and the
    /// model's precondition check, which is where a real one happens.
    LostRace {
        /// The key the competitor takes, decoded as the model stores it.
        key: String,
        /// What the competitor wrote, so a test can prove whose bytes survived.
        bytes: Vec<u8>,
    },
    /// The request is executed and its **response** is withheld for `holding`.
    ///
    /// ⚠️ **After the work, not instead of it — that is the whole distinction
    /// from a kill.** A killed node did nothing; a paused one did everything
    /// and said nothing, so the caller's deadline expires over a change that
    /// has already happened. Every metastable finding in the peer-system audit
    /// (doc 13 §"Metastable failure via lease expiry") came from that shape,
    /// and a harness that only kills is complete against the easy case.
    ///
    /// ⚠️ **The caller's deadline is the test's, not the client's.** A custom
    /// `HttpConnector` bypasses `ClientOptions::timeout` entirely — that
    /// 30-second default is applied by `reqwest`'s builder, which this model
    /// replaces — and `RetryConfig::retry_timeout` is only consulted *between*
    /// attempts, so nothing in `object_store` cuts a single hanging request
    /// here. ⚠️ **That is a fidelity gap between `sim` and `s3_minio`, stated
    /// rather than papered over**: against real S3 a pause this long is cut at
    /// 30 s by the client, and here it is unbounded, so a case that wants a
    /// deadline supplies its own — which is what `connection.rs`'s
    /// `tokio::time::timeout` does in the code this models.
    Pause {
        /// Matched against the request's method verbatim, as `Storm`'s is.
        method: &'static str,
        /// How long the response is withheld, in virtual time.
        holding: core::time::Duration,
        /// How many more matching requests are paused, decremented per use.
        remaining: u32,
    },
    /// A bulk delete naming `key` reports a per-key `<Error>` with this code,
    /// and the object stays.
    ///
    /// ⚠️ **Standing rather than one-shot.** The failure being modelled is a
    /// key that cannot be deleted — a lifecycle hold, a bucket policy — not a
    /// transient hiccup, and a one-shot version would be indistinguishable
    /// from a storm the caller retried through.
    RefusedDelete {
        /// The key the delete refuses.
        key: String,
        /// The S3 error code the `<Error>` element carries.
        code: &'static str,
    },
}

impl std::fmt::Debug for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storm { method, remaining } => f
                .debug_struct("Storm")
                .field("method", method)
                .field("remaining", remaining)
                .finish(),
            Self::LostRace { key, bytes } => f
                .debug_struct("LostRace")
                .field("key", key)
                .field("bytes", &format_args!("{} bytes", bytes.len()))
                .finish(),
            Self::Pause {
                method,
                holding,
                remaining,
            } => f
                .debug_struct("Pause")
                .field("method", method)
                .field("holding", holding)
                .field("remaining", remaining)
                .finish(),
            Self::RefusedDelete { key, code } => f
                .debug_struct("RefusedDelete")
                .field("key", key)
                .field("code", code)
                .finish(),
        }
    }
}

/// The faults armed on one model, consulted per request.
#[derive(Debug, Default)]
pub(crate) struct Faults {
    armed: Vec<Fault>,
    injected: u32,
}

impl Faults {
    /// Adds a fault to the plan.
    pub(crate) fn arm(&mut self, fault: Fault) {
        self.armed.push(fault);
    }

    /// How many requests have actually been answered with a fault.
    ///
    /// ⚠️ **Without this every fault test can pass vacuously**, and the
    /// absorbed-storm case is the one that shows why: a storm that never fired
    /// leaves the PUT succeeding, which is exactly what the test asserts. So a
    /// case says how many faults it expects to be met, and an `inject` that
    /// silently matched nothing fails it.
    pub(crate) const fn injected(&self) -> u32 {
        self.injected
    }

    /// The status a storm owes this method, consuming one of its requests.
    pub(crate) fn storm(&mut self, method: &str) -> Option<u16> {
        let answered = self.armed.iter_mut().find_map(|fault| match fault {
            Fault::Storm {
                method: m,
                remaining,
            } if *m == method && *remaining > 0 => {
                *remaining -= 1;
                Some(SERVICE_UNAVAILABLE)
            }
            _ => None,
        });
        self.injected += u32::from(answered.is_some());
        answered
    }

    /// How long this request's response is withheld, consuming one pause.
    pub(crate) fn pause(&mut self, method: &str) -> Option<core::time::Duration> {
        let held = self.armed.iter_mut().find_map(|fault| match fault {
            Fault::Pause {
                method: m,
                holding,
                remaining,
            } if *m == method && *remaining > 0 => {
                *remaining -= 1;
                Some(*holding)
            }
            _ => None,
        });
        self.injected += u32::from(held.is_some());
        held
    }

    /// The bytes a competitor lands under `key` before this PUT is evaluated.
    ///
    /// ⚠️ **Removed when taken**, because a race happens once: leaving it
    /// armed would make every later write to that key lose, which is a
    /// permanently unwritable key and a different fault.
    pub(crate) fn lost_race(&mut self, key: &str) -> Option<Vec<u8>> {
        let at = self
            .armed
            .iter()
            .position(|fault| matches!(fault, Fault::LostRace { key: k, .. } if k == key))?;
        let Fault::LostRace { bytes, .. } = self.armed.remove(at) else {
            unreachable!("the index came from a LostRace match")
        };
        self.injected += 1;
        Some(bytes)
    }

    /// The S3 error code a delete of `key` is refused with, if any.
    pub(crate) fn refusal(&mut self, key: &str) -> Option<&'static str> {
        let code = self.armed.iter().find_map(|fault| match fault {
            Fault::RefusedDelete { key: k, code } if k == key => Some(*code),
            _ => None,
        });
        self.injected += u32::from(code.is_some());
        code
    }
}

/// Refuses a run whose schedule a seed cannot pin.
///
/// ⚠️ **The decision `M10.6` left open, enforced rather than written down.**
/// A `multi_thread` runtime hands one model's requests to two workers, which
/// take latency draws and consume storm counts in arrival order — so one seed
/// gives two runs, silently, and the harness becomes the "usually
/// deterministic" thing `M10.md`'s first risk names. `ADR-0028`'s
/// `current_thread` is what buys the ordering.
///
/// ⚠️ **So latency and an *injected* fault do not reach the conformance run,
/// and cannot.** ⚠️ **`arm_ack_loss` is the exception and is armed by that run
/// today** — its own doc carries why that is safe there, and the argument is
/// about one request being in flight rather than about the flavour, so it does
/// not generalise to a fourth entry point.
/// `conformance::block_on` drives its cases with a `Waker::noop` spin loop
/// rather than `.await`: on one thread that loop starves the reactor the S3
/// client needs, which is why `the_simulated_backend_passes_the_full_conformance_suite`
/// and `s3_minio.rs` are both `multi_thread` — and under `start_paused` a
/// spinning task never lets the runtime idle, so the virtual clock would never
/// advance and an injected delay would hang rather than pass. A fault run is
/// therefore its own test, and this assertion is what keeps that a decision
/// rather than something a later edit undoes by moving one attribute.
pub(crate) fn require_a_single_threaded_runtime(what: &str) {
    assert_eq!(
        tokio::runtime::Handle::current().runtime_flavor(),
        tokio::runtime::RuntimeFlavor::CurrentThread,
        "{what} needs `#[tokio::test]`'s current-thread runtime: two workers \
         take one model's draws in arrival order, so one seed gives two \
         schedules and a failing run does not replay"
    );
}

// ── The cases that drive them ───────────────────────────────────────────────
//
// ⚠️ **Beside the framework rather than in `sim.rs`**, which is where they were
// written and where they took that file past `code-structure.md`'s 500-line
// limit. The seam the limit made visible is a real one: every case below is
// about a fault, and reads with the `Fault` it arms rather than with the
// round-trip cases `sim.rs` keeps. `latency.rs` is arranged the same way.

use super::model::ModelS3;
use super::{key, store_over};
use oqueue_core::{ByteRange, Error, ObjectStore, Precondition};

/// ⚠️ **Where the shipped retry budget actually ends, measured rather than
/// translated.** `retry.rs` pins that `RetryPolicy::DEFAULT` becomes
/// `max_retries: 2`; what no test could reach until faults existed is that two
/// retries means a *third* attempt, so a storm two requests deep is survived
/// and the caller never learns of it.
#[tokio::test(start_paused = true)]
async fn a_storm_inside_the_retry_budget_is_absorbed() {
    let model = ModelS3::default();
    model.inject(Fault::Storm {
        method: "PUT",
        remaining: 2,
    });
    let store = store_over(&model);
    let k = key("stormed/survivor");

    store
        .put(&k, b"through".to_vec(), None)
        .await
        .expect("two 503s are inside a three-attempt budget");

    assert_eq!(
        model.faults_injected(),
        2,
        "both 503s must have been answered — a storm that fired at nothing \
         leaves this test asserting only that a plain put works"
    );

    assert_eq!(
        store.get(&k, ByteRange::Full).await,
        Ok(b"through".to_vec()),
        "the surviving attempt must have stored the bytes"
    );
}

/// ⚠️ **One request further and it is the caller's problem** — as `Transient`,
/// which is `classify.rs`'s `Generic` arm and the class a caller's own
/// `RetryPolicy::decide` acts on. ⚠️ **And nothing landed**: a storm that
/// answered 503 *after* storing would make this a lost ack rather than a
/// refused write, and the two have opposite consequences under `ADR-0005`.
#[tokio::test(start_paused = true)]
async fn a_storm_past_the_retry_budget_surfaces_as_transient() {
    let model = ModelS3::default();
    model.inject(Fault::Storm {
        method: "PUT",
        remaining: 3,
    });
    let store = store_over(&model);
    let k = key("stormed/lost");

    let err = store
        .put(&k, b"never".to_vec(), None)
        .await
        .expect_err("three 503s exhaust a three-attempt budget");

    assert_eq!(model.faults_injected(), 3, "three attempts, three 503s");
    assert_eq!(err, Error::Transient);
    assert_eq!(
        store.get(&k, ByteRange::Full).await,
        Err(Error::ObjectNotFound { key: k }),
        "a refused write must not have stored anything"
    );
}

/// ⚠️ **The race `a_second_if_absent_write_loses_the_race` cannot stage.**
/// That case writes the key first, so the loser was always going to lose;
/// here the key is absent when the caller decides and taken before the model
/// checks, which is the window a real commit loses in. The observable outcome
/// is the same `PreconditionFailed` — and that it *is* the same is the claim:
/// a caller cannot distinguish the two, so `ADR-0005`'s protocol may not
/// depend on doing so.
#[tokio::test]
async fn a_write_that_loses_a_planted_race_is_refused_like_any_other() {
    let model = ModelS3::default();
    let k = key("contended/window");
    model.inject(Fault::LostRace {
        key: "contended/window".to_owned(),
        bytes: b"competitor".to_vec(),
    });
    let store = store_over(&model);

    let err = store
        .put(&k, b"mine".to_vec(), Some(Precondition::IfAbsent))
        .await
        .expect_err("the competitor took the key inside the window");

    assert_eq!(
        model.faults_injected(),
        1,
        "the race must have been planted"
    );
    assert_eq!(err, Error::PreconditionFailed { key: k.clone() });
    assert_eq!(
        store.get(&k, ByteRange::Full).await,
        Ok(b"competitor".to_vec()),
        "the winner's bytes are what survive, and the loser's are not written"
    );
}

/// ⚠️ **`delete` is not atomic across the keys it is given**, which no test
/// could observe before a delete could partially fail. `s3.rs` loops per key
/// and returns on the first failure, so a caller that treats `delete(&[a, b])`
/// as all-or-nothing is wrong in both directions: `a` is gone, `b` is not, and
/// the `Err` says nothing about which.
///
/// ⚠️ **And the class is `Transient`, because the code S3 sent is erased before
/// `classify.rs` ever sees it.** `object_store`'s public `Error` has **no**
/// variant for a per-key delete failure — twelve variants, none of them one —
/// so there is nothing for `classify.rs` to match on. `aws/client.rs` builds a
/// crate-private `DeleteFailed` carrying the key, the code and the message,
/// and all but two variants of that enum convert to `crate::Error::Generic`
/// with the original boxed as an opaque source. So an `AccessDenied` hold no
/// retry can fix is retried, bounded, by every caller obeying `retry_class()`,
/// and `error-handling.md` rule 8 forbids the one route to the code that is
/// left. `M10.28` is the row for deciding what to do about it; this test pins
/// today's answer so the decision is a change rather than a discovery.
#[tokio::test]
async fn a_refused_delete_stops_the_loop_with_the_earlier_keys_already_gone() {
    let model = ModelS3::default();
    let (first, held) = (key("gc/first"), key("gc/held"));
    model.inject(Fault::RefusedDelete {
        key: "gc/held".to_owned(),
        code: "AccessDenied",
    });
    let store = store_over(&model);
    for k in [&first, &held] {
        store
            .put(k, b"bytes".to_vec(), None)
            .await
            .expect("the put succeeds");
    }

    let err = store
        .delete(&[first.clone(), held.clone()])
        .await
        .expect_err("the second key is refused");

    assert_eq!(model.faults_injected(), 1, "one key refused, and only one");
    assert_eq!(err, Error::Transient);
    assert_eq!(
        store.get(&first, ByteRange::Full).await,
        Err(Error::ObjectNotFound { key: first }),
        "the key before the refusal was deleted, and stays deleted"
    );
    assert_eq!(
        store.get(&held, ByteRange::Full).await,
        Ok(b"bytes".to_vec()),
        "the refused key is still there, which is what makes it a refusal"
    );
}

/// ⚠️ **The decision `M10.6` left open, as a test rather than a paragraph.**
/// Two workers take one model's storm counts and latency draws in arrival
/// order, so a seed pins nothing — and the failure would be a harness that is
/// *usually* deterministic, which `M10.md`'s first risk names as worse than
/// one that is honestly not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "current-thread runtime")]
async fn a_fault_armed_on_a_multi_thread_runtime_is_refused() {
    ModelS3::default().inject(Fault::Storm {
        method: "PUT",
        remaining: 1,
    });
}

/// ⚠️ **Two of the three entry points, guarded separately**, because a guard on
/// one is one a later test walks around without noticing. ⚠️ **The third is
/// `arm_ack_loss`, which is not guarded** — the conformance run arms it and is
/// `multi_thread` by necessity — and its own doc carries the argument for why
/// that is safe there and would not be safe generally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "current-thread runtime")]
async fn latency_on_a_multi_thread_runtime_is_refused() {
    let _model = ModelS3::default().with_latency(1);
}
