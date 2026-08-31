//! A deterministic S3, answered in-process, and `S3Store` driven over it.
//!
//! ⚠️ **Here rather than in `oqueue-testkit`, and here rather than behind a
//! `src/` feature** — `ADR-0027` point 2. `oqueue-store` is the only crate
//! that depends on `object_store`, so its own tests are this seam's only
//! consumer; `check-sans-io.sh`'s exemptions are directory prefixes, so a
//! `tests/` tree is exempt exactly as `src/` is; and `check-crate.sh` runs
//! `cargo test` with **default** features, so a feature-gated transport would
//! compile under clippy and be executed by nothing.
//!
//! ⚠️ **What this buys is the seam-crossing the unit tests cannot reach.**
//! `classify.rs` already tests its mapping against synthetic
//! `object_store::Error` values, and that is a different claim: this drives a
//! real *status code* through `object_store`'s own response handling and out
//! as one of ours, which until now needed `MinIO` and a Docker daemon.
//!
//! ⚠️ **What it does not buy** (`testing.md`, "Deterministic simulation"): the
//! socket, TLS, connection pooling, or `object_store`'s own retry timing. Those
//! stay T2's, which is why `s3_minio.rs` keeps every case it has.
//!
//! The other two pieces of the harness are `oqueue-testkit` (seed, schedule,
//! invariants) and `crates/oqueue-broker/tests/` (the simulated socket).

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod model;

use crate::conformance::{
    Capabilities, Harness, record::record_backend_run, run_conformance_suite,
};
use model::{BUCKET, ModelS3};
use object_store::aws::AmazonS3Builder;
use oqueue_core::RetryPolicy;
use oqueue_core::{ByteRange, Error, ObjectKey, ObjectStore, Precondition};
use oqueue_store::{S3Store, retry_config_for};

/// An `S3Store` whose every request is answered by `model`.
fn store_over(model: &ModelS3) -> S3Store {
    let inner = AmazonS3Builder::new()
        .with_bucket_name(BUCKET)
        .with_region("us-east-1")
        .with_access_key_id("sim")
        .with_secret_access_key("sim")
        .with_endpoint("http://sim.invalid")
        .with_allow_http(true)
        // ⚠️ **The production retry budget, not the vendor default.** Review
        // measured the gap: `RetryConfig::default()` is `max_retries: 10` and
        // `retry_config_for(RetryPolicy::DEFAULT)` is `max_retries: 2` — so
        // **eleven attempts against three**, which is the conversion
        // `retry_config_for`'s own doc calls the whole of the translation that
        // could be wrong, and which an earlier draft of this comment got wrong
        // by calling retries attempts. A model absorbing a storm eight
        // attempts longer than the shipped client would let `M10.7` — the row
        // that builds 503 storms — "prove" survival of a storm no deployment
        // survives.
        .with_retry(retry_config_for(RetryPolicy::DEFAULT))
        .with_http_connector(model.clone())
        .build()
        .expect("the model builder is fully specified");
    S3Store::from_client(inner)
}

/// ⚠️ **The whole conformance suite, against a backend that is not a fake.**
/// `M10.3`. The suite is `oqueue-store`'s own description of what an
/// `ObjectStore` must do, and holding the model to it is the alternative to
/// writing a second description of the same contract and hoping they agree.
///
/// ⚠️ **`Capabilities::FULL`, which `s3_minio.rs` cannot declare.** Nothing can
/// tell S3 to lose an acknowledgement after a durable write, so that backend
/// declares `injectable_ack_loss: false` and the case is skipped and recorded.
/// A model *can* be told — `arm_ack_loss` — so this is the first backend
/// besides the in-memory fake to run `a_failed_put_is_not_proof_of_absence`,
/// which is `ADR-0005` guarantee 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_simulated_backend_passes_the_full_conformance_suite() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let arm = || model.arm_ack_loss();
    let harness = Harness::new(&store).with_crash_after_put(&arm);

    let report = run_conformance_suite("sim", &harness, Capabilities::FULL);

    assert_eq!(report.backend_name, "sim");
    assert!(
        report.skipped.is_empty(),
        "the model declares every capability, so nothing should be skipped: {:?}",
        report.skipped
    );
    assert!(
        !report.ran.is_empty(),
        "the suite must have actually run at least one case"
    );
    // ⚠️ **Named, not counted.** `M10.22` is the row for a leg that printed
    // "ack-loss case included" over a test asserting only that nothing was
    // skipped; this is the same claim made where it can be checked. ⚠️ **That
    // row is not discharged by this line**: `fake.rs` still asserts nothing
    // about `report.ran`, and `m3-complete.sh` reads `fake.rs`.
    assert!(
        report.ran.contains(&"a_failed_put_is_not_proof_of_absence"),
        "the case S3 must skip is the reason this backend exists: {:?}",
        report.ran
    );

    record_backend_run("sim");
}

/// ⚠️ **The error *class*, which the suite cannot see.**
/// `a_failed_put_is_not_proof_of_absence` asserts only `is_err()`, so the
/// status the model answers for an injected ack loss survived mutation: 403
/// classifies as `Permanent` — `RetryClass::Never` — where the fake gives
/// `Transient`, and the one case both backends advertise as running would have
/// produced opposite retry classes with every test green. A caller obeying
/// `retry_class()` would give up on a write that actually landed.
#[tokio::test]
async fn an_injected_ack_loss_is_retriable_exactly_as_the_fake_makes_it() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let k = key("ack/lost");

    model.arm_ack_loss();
    let err = store
        .put(&k, b"durable".to_vec(), None)
        .await
        .expect_err("the injected loss makes this put fail");

    assert_eq!(err, Error::Transient);
    // And the bytes are there, which is what makes it a *lost ack* rather
    // than a failed write — `ADR-0005` guarantee 2.
    assert_eq!(
        store.get(&k, ByteRange::Full).await,
        Ok(b"durable".to_vec())
    );
}

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a valid key")
}

/// ⚠️ **The claim is that a status code becomes one of *our* errors**, which no
/// unit test reaches: `classify.rs` starts from an `object_store::Error` that a
/// test constructed, and this starts from bytes on a wire.
#[tokio::test]
async fn an_object_written_through_the_model_reads_back_byte_for_byte() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let k = key("round/trip");

    store
        .put(&k, b"hello".to_vec(), None)
        .await
        .expect("the put succeeds");
    let got = store
        .get(&k, ByteRange::Full)
        .await
        .expect("the get succeeds");

    assert_eq!(got, b"hello".to_vec());
}

#[tokio::test]
async fn a_missing_object_is_not_found_rather_than_an_empty_read() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let k = key("absent");

    let err = store
        .get(&k, ByteRange::Full)
        .await
        .expect_err("nothing is stored");

    assert_eq!(err, Error::ObjectNotFound { key: k });
}

/// ⚠️ **The case that caught the dead `DELETE` arm.** `S3Store::delete` goes
/// out as a bulk `POST ?delete`, so a model answering only `DELETE` removed
/// nothing and reported `Transient` — and no test here called `delete`, which
/// is why the branch read as working code for a round.
#[tokio::test]
async fn a_deleted_object_is_gone_and_deleting_it_again_is_not_an_error() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let k = key("removed");

    store
        .put(&k, b"bytes".to_vec(), None)
        .await
        .expect("the put succeeds");
    store
        .delete(std::slice::from_ref(&k))
        .await
        .expect("the delete succeeds");

    let err = store
        .get(&k, ByteRange::Full)
        .await
        .expect_err("the object is gone");
    assert_eq!(err, Error::ObjectNotFound { key: k.clone() });

    // ⚠️ Idempotent, which is `conformance.rs`'s claim and S3's behaviour: a
    // key that was never there is reported deleted rather than refused.
    store
        .delete(&[k])
        .await
        .expect("deleting an absent key is not an error");
}

/// ⚠️ **A key spelled one way in a URL and another in XML.** `ObjectKey::new`
/// accepts any non-empty string, and `object_store` percent-encodes a path
/// while sending the raw key in a bulk-delete body — so the model kept two
/// spellings of one object and answered `<Deleted>` for a delete that removed
/// nothing. Every key in `conformance.rs` is drawn from the unreserved set,
/// which is why the suite `M10.3` runs could not have caught it.
#[tokio::test]
async fn a_key_the_url_escapes_is_deleted_under_the_name_it_was_stored_by() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let k = key("space and &ersand");

    store
        .put(&k, b"present".to_vec(), None)
        .await
        .expect("the put succeeds");
    store
        .delete(std::slice::from_ref(&k))
        .await
        .expect("the delete succeeds");

    let err = store
        .get(&k, ByteRange::Full)
        .await
        .expect_err("the object is gone, not merely reported gone");
    assert_eq!(err, Error::ObjectNotFound { key: k });
}

/// ⚠️ **The case that caught the dead 416 branch**, and the reason it is here
/// rather than left to `M10.3`: the model's own comment claimed this mapping
/// worked while nothing exercised it.
#[tokio::test]
async fn a_range_starting_past_the_object_is_out_of_bounds() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let k = key("short");

    store
        .put(&k, b"abc".to_vec(), None)
        .await
        .expect("the put succeeds");
    let err = store
        .get(&k, ByteRange::bounded(10, 5).expect("a valid range"))
        .await
        .expect_err("the range starts past the object");

    assert_eq!(
        err,
        Error::ByteRangeOutOfBounds {
            key: k,
            offset: 10,
            length: 5,
            object_size: 3,
        }
    );
}

/// ⚠️ **The 412 the whole file is for.** `ADR-0005` makes the conditional write
/// the primitive every commit rests on, and until now the only thing that could
/// produce a real one was `MinIO` behind a Docker daemon. ⚠️ This paragraph sat
/// on the delete case for one round, where it claimed a 412 no request sent.
#[tokio::test]
async fn a_second_if_absent_write_loses_the_race() {
    let model = ModelS3::default();
    let store = store_over(&model);
    let k = key("contended");

    store
        .put(&k, b"first".to_vec(), Some(Precondition::IfAbsent))
        .await
        .expect("the first write wins");
    let err = store
        .put(&k, b"second".to_vec(), Some(Precondition::IfAbsent))
        .await
        .expect_err("the second write loses");

    assert_eq!(err, Error::PreconditionFailed { key: k });
}
