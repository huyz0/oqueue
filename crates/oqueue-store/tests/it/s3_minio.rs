//! The S3 backend against a real `MinIO` container — `M1.15`.
//!
//! ⚠️ **T2, not T0/T1** (`testing.md`'s tier table): needs Docker and the
//! network, so it is `#[ignore]`d rather than gated by a feature or a
//! runtime environment check (`testing.md` rule 2 — capability, never
//! `#[cfg]`). Whatever runs it starts `MinIO` and sets every `AWS_*`
//! variable this backend reads **before the test process launches**, then
//! runs with `--ignored` — this file never calls `std::env::set_var` itself
//! (`testing.md` rule 12: mutating the environment at runtime is unsafe and
//! racy against every other test in the same binary). A developer without
//! Docker running locally never opts in; nothing here needs remembering to
//! turn off.
//!
//! ⚠️ **Two things run it: `scripts/gates/m1-complete.sh` and CI's
//! `conformance-t2` job.** This sentence used to say "CI's T2 step" does —
//! `M1.21` went looking for that step in order to read its bucket setup and
//! found that `.github/workflows/gates.yml` had no T2 job and never had one.
//! `M1.34` wrote it.
//! ⚠️ **That is not the same as never having run**: `M1.15` and `M1.16` both
//! record running these tests against a real `MinIO` container in their commit
//! messages, and `check-coverage.sh`'s exemption for this crate rests on that
//! measurement. What was missing is anything that re-runs them — between one
//! person's invocation and the next they were unenforced. The milestone gate
//! starts the container, creates the bucket, exports the credentials and runs
//! the suite twice, which makes that invocation repeatable — and `M1.34`'s
//! `conformance-t2` job does the same on every push, so between one person's
//! invocation and the next these are no longer unenforced. ⚠️ Both run the
//! suite **twice against one bucket**, which is the property that matters
//! rather than an incidental repetition: `M1.21` found a case asserting a key
//! absent and then creating it without cleanup, which passes on an empty
//! bucket and fails on the second run.
//!
//! ⚠️ **Must run on a multi-thread Tokio runtime.** The conformance suite's
//! own case bodies drive their futures with a hand-rolled, no-allocation
//! busy-poll loop (`conformance::block_on`) instead of `.await` — exactly
//! right for `oqueue-core`'s and the fake's zero-I/O cases, since nothing
//! there ever needs a reactor. `object_store`'s S3 client polls real
//! sockets, which need Tokio's I/O driver actually turning. On a
//! `current_thread` runtime the one OS thread never reaches that turn while
//! stuck spinning this crate's busy-poll loop; `multi_thread` leaves other
//! worker threads free to drive it while this task spins. `oqueue-store`'s
//! own `Cargo.toml` pins `rt-multi-thread` for exactly this reason.

// The workspace denies `expect_used`; the two sites below are on a
// precondition whatever runs this suite is responsible for holding (`AWS_*`
// describing a reachable endpoint) before either test starts — a panic here
// means the test environment is broken, not that this test should recover.
#![allow(clippy::expect_used)]

use crate::conformance::{Capabilities, record::record_backend_run, run_conformance_suite};
use oqueue_core::{ByteRange, Error, MultipartLimits, ObjectKey, ObjectStore, Precondition};
use oqueue_store::S3Store;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "T2: run by CI's conformance-t2 job and by scripts/gates/m1-complete.sh"]
async fn s3_backend_passes_the_full_conformance_suite_against_minio() {
    let store = S3Store::from_env().expect(
        "AWS_* environment variables must describe a reachable MinIO endpoint \
         when this test is run with --ignored",
    );
    let report = run_conformance_suite("s3", &store, Capabilities::FULL);

    assert_eq!(report.backend_name, "s3");
    assert!(
        report.skipped.is_empty(),
        "S3Store declares every capability, so nothing should be skipped: {:?}",
        report.skipped
    );
    assert!(
        !report.ran.is_empty(),
        "the suite must have actually run at least one case"
    );

    record_backend_run("s3");
}

/// `M1.16` — real multipart, against a real endpoint, not just the pure
/// chunk-planning `s3.rs`'s own unit tests already cover.
///
/// ⚠️ **`with_multipart_limits` lowers the trigger threshold to 5 MiB**
/// (still S3's real *minimum* part size, so every part this test actually
/// sends is protocol-valid) **rather than leaving it at
/// `S3_MULTIPART_LIMITS`'s real 5 GiB** — this test would otherwise need a
/// multi-gigabyte payload to ever reach the multipart path at all. See
/// `S3Store::with_multipart_limits`'s own doc comment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "T2: run by CI's conformance-t2 job and by scripts/gates/m1-complete.sh"]
async fn s3_backend_uploads_a_large_payload_as_multipart_and_reads_it_back() {
    let limits = MultipartLimits {
        min_part_size: 5 * 1024 * 1024,
        max_part_size: 5 * 1024 * 1024,
        max_parts: 10,
        max_object_size: 100 * 1024 * 1024,
        // Unused by this unconditional upload; S3's real value, so the only
        // lowered numbers are the ones this test is about.
        max_single_put: 5 * 1024 * 1024 * 1024,
    };
    let store = S3Store::from_env()
        .expect(
            "AWS_* environment variables must describe a reachable MinIO endpoint \
             when this test is run with --ignored",
        )
        .with_multipart_limits(limits);

    let key = ObjectKey::new("s3-minio/multipart.seg").expect("a non-empty key");
    // 11 MiB: two full 5 MiB parts plus a 1 MiB remainder -- proves the
    // upload actually split into parts, not merely that one large payload
    // happens to round-trip.
    let mut payload = vec![0u8; 11 * 1024 * 1024];
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte = u8::try_from(i % 256).unwrap_or(0);
    }

    let meta = store
        .put(&key, payload.clone(), None)
        .await
        .expect("a multipart put against real MinIO succeeds");
    assert_eq!(meta.size, u64::try_from(payload.len()).unwrap_or(u64::MAX));

    let read_back = store
        .get(&key, ByteRange::Full)
        .await
        .expect("reading a multipart-uploaded object back succeeds");
    assert_eq!(
        read_back, payload,
        "every byte of a multipart upload must round-trip, not just its length"
    );

    // ADR-0013 still holds -- a precondition can never ride a multipart
    // completion. What `M2.3` moved is the bound it fires at: the refusal
    // now triggers on `max_single_put`, not the chunking preference. With
    // this test's limits (the real 5 GiB ceiling), an 11 MiB conditional
    // put is a *single* request -- and against the key this test just
    // created, `IfAbsent` fails at the backend, proving the request was
    // genuinely sent rather than refused locally.
    let result = store
        .put(&key, payload.clone(), Some(Precondition::IfAbsent))
        .await;
    assert_eq!(
        result,
        Err(Error::PreconditionFailed { key: key.clone() }),
        "an over-part-size conditional put is a real single request (M2.3)"
    );

    store
        .delete(std::slice::from_ref(&key))
        .await
        .expect("cleaning up the multipart-uploaded object succeeds");
}

/// The loud refusal at the bound it actually lives at (`M2.3`): a payload
/// over `max_single_put` turns a conditional put into `Err(Permanent)`
/// before any request is made -- locally, so this cannot flake on the
/// network even though it runs in the T2 group for the same `from_env`
/// environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "T2: run by CI's conformance-t2 job and by scripts/gates/m1-complete.sh"]
async fn s3_backend_refuses_a_conditional_put_over_the_single_request_ceiling() {
    let refusing = S3Store::from_env()
        .expect(
            "AWS_* environment variables must describe a reachable MinIO endpoint \
             when this test is run with --ignored",
        )
        .with_multipart_limits(MultipartLimits {
            min_part_size: 5 * 1024 * 1024,
            max_part_size: 5 * 1024 * 1024,
            max_parts: 10,
            max_object_size: 100 * 1024 * 1024,
            max_single_put: 5 * 1024 * 1024,
        });
    let key = ObjectKey::new("s3-minio/refused.seg").expect("a non-empty key");
    let payload = vec![0u8; 6 * 1024 * 1024];
    let result = refusing
        .put(&key, payload, Some(Precondition::IfAbsent))
        .await;
    assert_eq!(result, Err(Error::Permanent));
}
