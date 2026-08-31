//! The cases for `Fault::Pause`: a response withheld after the work is done.
//!
//! ⚠️ **Split from `faults.rs` at `M10.8`, on the seam the row is named for.**
//! A storm and a refused delete are *answers* — the backend says no, loudly,
//! and the caller learns something true. A pause says nothing, and everything
//! the caller then believes is wrong. Every metastable finding in the
//! peer-system audit came from that second shape (doc 13
//! §"Metastable failure via lease expiry"), which is why it is worth reading
//! as its own file rather than as three more entries in a fault list.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::faults::Fault;
use super::model::ModelS3;
use super::{key, store_over};
use core::time::Duration;
use oqueue_core::{ByteRange, Error, ObjectStore, Precondition};

/// How long a caller in these cases is willing to wait.
///
/// ⚠️ **The test's number, not the client's**, because nothing in
/// `object_store` bounds a single request reaching a custom `HttpConnector` —
/// see `Fault::Pause`. It stands for `connection.rs`'s `idle_timeout`: a
/// deadline the caller owns and the dependency knows nothing about.
const CALLER_DEADLINE: Duration = Duration::from_secs(5);

/// ⚠️ **The metastable shape, in the smallest form this tree can reach it.**
/// The write lands, the answer is withheld past the caller's deadline, and the
/// caller is left believing it failed — `ADR-0005` guarantee 2 arrived at by a
/// *pause* rather than by `arm_ack_loss` telling the model what to do. That is
/// the difference between a harness that reproduces a known case and one that
/// produces it: nothing here names an ack, and the caller's error is a
/// deadline rather than a status.
#[tokio::test(start_paused = true)]
async fn a_pause_the_caller_outwaits_leaves_a_write_it_believes_failed() {
    let model = ModelS3::default();
    model.inject(Fault::Pause {
        method: "PUT",
        holding: Duration::from_mins(5),
        remaining: 1,
    });
    let store = store_over(&model);
    let k = key("paused/landed");

    let gave_up = tokio::time::timeout(
        CALLER_DEADLINE,
        store.put(&k, b"landed".to_vec(), Some(Precondition::IfAbsent)),
    )
    .await;

    assert_eq!(model.faults_injected(), 1, "the pause must have been met");
    assert!(gave_up.is_err(), "the caller's deadline must expire first");
    assert_eq!(
        store.get(&k, ByteRange::Full).await,
        Ok(b"landed".to_vec()),
        "the write happened before the answer was withheld, which is what \
         makes this a pause and not a kill"
    );
}

/// ⚠️ **And the retry cannot tell its own write from a competitor's.** This is
/// the cascade the pause causes rather than the pause itself: the caller
/// believes its conditional write failed, retries the only way it can, and is
/// refused by the object *it* wrote. ⚠️ **`M10.7` pinned that a planted race
/// and a staged one are indistinguishable; this is the third thing that looks
/// identical**, so a commit protocol reading `PreconditionFailed` as "someone
/// else got there" is wrong here — and `ADR-0005`'s does not, which is the
/// claim this makes checkable.
#[tokio::test(start_paused = true)]
async fn the_retry_after_a_pause_is_refused_by_the_write_the_pause_hid() {
    let model = ModelS3::default();
    model.inject(Fault::Pause {
        method: "PUT",
        holding: Duration::from_mins(5),
        remaining: 1,
    });
    let store = store_over(&model);
    let k = key("paused/retried");
    let attempt = || store.put(&k, b"mine".to_vec(), Some(Precondition::IfAbsent));

    let gave_up = tokio::time::timeout(CALLER_DEADLINE, attempt()).await;
    assert!(gave_up.is_err(), "the first attempt outlasts the deadline");

    let refused = attempt()
        .await
        .expect_err("the retry meets the object the first attempt wrote");

    assert_eq!(refused, Error::PreconditionFailed { key: k });
    assert_eq!(
        model.faults_injected(),
        1,
        "one pause, spent on the first attempt — the retry is not paused"
    );
}

/// ⚠️ **A pause inside the deadline is only slow**, and the clock proves the
/// wait happened: a `Pause` that returned without sleeping would leave both
/// cases above passing for the wrong reason, since a caller that never waits
/// also never times out.
#[tokio::test(start_paused = true)]
async fn a_pause_the_caller_waits_out_is_only_slow() {
    let model = ModelS3::default();
    let holding = Duration::from_secs(1);
    model.inject(Fault::Pause {
        method: "PUT",
        holding,
        remaining: 1,
    });
    let store = store_over(&model);
    let started = tokio::time::Instant::now();

    store
        .put(&key("paused/patient"), b"slow".to_vec(), None)
        .await
        .expect("the caller outwaits the pause");

    assert_eq!(model.faults_injected(), 1);
    assert!(
        started.elapsed() >= holding,
        "the pause took {:?} of simulated time, so it was not waited at all",
        started.elapsed()
    );
}

/// ⚠️ **The method discriminator, which nothing else owns.** Every other pause
/// case arms `PUT` and issues a PUT first, so a `Faults::pause` that matched
/// any method would answer them identically — measured: weakening the guard
/// leaves the suite green without this. Here the write must go through
/// untouched and the *read* must be the thing held, which only a working
/// discriminator produces.
///
/// ⚠️ **`Storm`'s discriminator is still unowned in the same way**, and that is
/// `M10.7`'s debt rather than this row's — recorded so it is a known hole
/// rather than an assumed guard.
#[tokio::test(start_paused = true)]
async fn a_pause_armed_on_reads_lets_the_write_through() {
    let model = ModelS3::default();
    let holding = Duration::from_secs(2);
    model.inject(Fault::Pause {
        method: "GET",
        holding,
        remaining: 1,
    });
    let store = store_over(&model);
    let k = key("paused/read");

    let before_write = tokio::time::Instant::now();
    store
        .put(&k, b"unpaused".to_vec(), None)
        .await
        .expect("the write is not what was armed");
    assert_eq!(
        before_write.elapsed(),
        Duration::ZERO,
        "a pause armed on GET must not be spent by a PUT"
    );

    let before_read = tokio::time::Instant::now();
    assert_eq!(
        store.get(&k, ByteRange::Full).await,
        Ok(b"unpaused".to_vec())
    );
    assert_eq!(model.faults_injected(), 1);
    assert!(
        before_read.elapsed() >= holding,
        "the read is what was held"
    );
}
