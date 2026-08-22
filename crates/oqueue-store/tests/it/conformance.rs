//! The backend-agnostic conformance suite: one body, run once per registered
//! backend, against `docs/researches/04-object-storage-s3-gcs.md` §6's
//! documented semantics.
//!
//! ⚠️ **No async runtime, not even under `[dev-dependencies]`** — same floor
//! as `oqueue-core`'s own seam tests (`M0.16`), and this is the first crate
//! downstream of it to need the same busy-poll driver.

// The workspace denies `expect_used`; every site here is on a value this
// suite just constructed from a literal it controls, so a panic means the
// suite itself is wrong.
#![allow(clippy::expect_used)]

pub mod record;

use oqueue_core::{ByteRange, Error, ObjectKey, ObjectStore, Precondition};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion on this thread — see the module doc for why
/// this exists instead of a runtime.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a non-empty key")
}

/// Which operations a backend actually supports.
///
/// A backend that cannot do something declares it here, and the suite skips
/// (and records) whichever cases need it — never silently passing a case
/// the backend never ran.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// `put`'s `precondition` parameter is honoured, not merely accepted.
    pub conditional_writes: bool,
    /// `get`'s `range` parameter reads a genuine slice, not always the whole
    /// object regardless of what was asked for.
    pub ranged_reads: bool,
}

impl Capabilities {
    /// Every capability this suite currently knows to ask about.
    pub const FULL: Self = Self {
        conditional_writes: true,
        ranged_reads: true,
    };
}

/// One case in the suite: a name, what capability (if any) it needs, and the
/// assertion itself.
struct Case {
    name: &'static str,
    requires: fn(Capabilities) -> bool,
    run: fn(&dyn ObjectStore),
}

/// What running the suite against one backend found.
///
/// Which cases actually ran, and which were skipped because the backend
/// declared it could not support them. ⚠️ **Both lists matter equally** — a
/// report with an empty `ran` list looks identical to one where every case
/// passed unless a caller checks it, which is exactly the silent-pass this
/// suite exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceReport {
    /// The backend this report is about.
    pub backend_name: &'static str,
    /// Names of cases that actually ran.
    pub ran: Vec<&'static str>,
    /// Names of cases skipped because a required capability was declared off.
    pub skipped: Vec<&'static str>,
}

/// One entry per case, densely -- the registry grows by one entry per case
/// instead of a five-line struct literal, which is what keeps `cases` inside
/// `too_many_lines` as the suite grows.
fn c(name: &'static str, requires: fn(Capabilities) -> bool, run: fn(&dyn ObjectStore)) -> Case {
    Case {
        name,
        requires,
        run,
    }
}

fn cases() -> Vec<Case> {
    vec![
        c("put_get_roundtrip", |_| true, put_get_roundtrip),
        c(
            "get_of_never_put_key_is_not_found",
            |_| true,
            get_of_never_put_key_is_not_found,
        ),
        c(
            "empty_object_is_distinct_from_missing",
            |_| true,
            empty_object_is_distinct_from_missing,
        ),
        c("delete_is_idempotent", |_| true, delete_is_idempotent),
        c(
            "conditional_write_if_absent",
            |c| c.conditional_writes,
            conditional_write_if_absent,
        ),
        c(
            "conditional_write_if_matches_succeeds_with_the_current_token",
            |c| c.conditional_writes,
            conditional_write_if_matches_succeeds_with_the_current_token,
        ),
        c(
            "conditional_write_if_matches_rejects_stale_token",
            |c| c.conditional_writes,
            conditional_write_if_matches_rejects_stale_token,
        ),
        c(
            "conditional_write_above_the_chunk_preference_is_one_request",
            |c| c.conditional_writes,
            conditional_write_above_the_chunk_preference_is_one_request,
        ),
        c(
            "ranged_get_starting_past_the_object_size_is_out_of_bounds",
            |c| c.ranged_reads,
            ranged_get_starting_past_the_object_size_is_out_of_bounds,
        ),
        c(
            "ranged_get_reads_exactly_the_requested_slice",
            |c| c.ranged_reads,
            ranged_get_reads_exactly_the_requested_slice,
        ),
    ]
}

/// Runs every case against `store`, skipping (and recording, in the
/// returned report) any case `capabilities` declares unsupported.
pub fn run_conformance_suite(
    backend_name: &'static str,
    store: &dyn ObjectStore,
    capabilities: Capabilities,
) -> ConformanceReport {
    let mut ran = Vec::new();
    let mut skipped = Vec::new();
    for case in cases() {
        if (case.requires)(capabilities) {
            (case.run)(store);
            ran.push(case.name);
        } else {
            skipped.push(case.name);
        }
    }
    ConformanceReport {
        backend_name,
        ran,
        skipped,
    }
}

fn put_get_roundtrip(store: &dyn ObjectStore) {
    let k = key("conformance/put_get_roundtrip.seg");
    let payload = vec![0x00, 0xff, 0x42, 0x00, 0x7f];
    block_on(store.put(&k, payload.clone(), None)).expect("put succeeds");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(payload));
}

fn get_of_never_put_key_is_not_found(store: &dyn ObjectStore) {
    let k = key("conformance/never_put.seg");
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Err(Error::ObjectNotFound { key: k })
    );
}

fn empty_object_is_distinct_from_missing(store: &dyn ObjectStore) {
    let k = key("conformance/empty.seg");
    block_on(store.put(&k, Vec::new(), None)).expect("put succeeds");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(Vec::new()));
}

fn delete_is_idempotent(store: &dyn ObjectStore) {
    let k = key("conformance/delete_idempotent.seg");
    block_on(store.put(&k, vec![1], None)).expect("put succeeds");
    block_on(store.delete(std::slice::from_ref(&k))).expect("first delete succeeds");
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Err(Error::ObjectNotFound { key: k.clone() }),
        "the object must actually be gone after delete, not merely report success"
    );
    block_on(store.delete(std::slice::from_ref(&k)))
        .expect("second delete, of an already-absent key, also succeeds");
}

/// `M2.3`, closing `M1.53`'s divergence: a conditional write's ceiling is
/// the backend's single-request bound (`max_single_put`), never its chunking
/// preference (`max_part_size`) -- before that fix, this exact payload was
/// `Ok` on S3 and `Err(Permanent)` on GCS, and no case could see it.
///
/// 12 MiB: above GCS's 8 MiB chunking preference, far below every backend's
/// single-request ceiling, and small enough to upload twice per gate run.
fn conditional_write_above_the_chunk_preference_is_one_request(store: &dyn ObjectStore) {
    let k = key("conformance/conditional_large.seg");
    // Delete-first, same reasoning as `conditional_write_if_absent`.
    block_on(store.delete(std::slice::from_ref(&k)))
        .expect("clearing anything a previous run left behind succeeds");
    let payload = vec![0xa5u8; 12 * 1024 * 1024];
    block_on(store.put(&k, payload.clone(), Some(Precondition::IfAbsent))).expect(
        "a conditional write above the chunk preference is a single request, not a refusal",
    );
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Ok(payload),
        "the large conditional write landed whole"
    );
}

/// `M2.4`, closing `M1.54`: a range starting at or past the object's size
/// is [`Error::ByteRangeOutOfBounds`] on every backend -- the fake decides
/// it locally; the real backends map the vendor's definitive 416, fetching
/// the size with a `HEAD` on that error path alone. Before the fix this
/// exact call was `Transient` on both real backends -- "retry with backoff"
/// for a range that can never satisfy -- and no case could see it.
fn ranged_get_starting_past_the_object_size_is_out_of_bounds(store: &dyn ObjectStore) {
    let k = key("conformance/past_size.seg");
    block_on(store.put(&k, vec![1, 2, 3], None)).expect("put succeeds");
    let range = ByteRange::bounded(10, 5).expect("a valid range");
    assert_eq!(
        block_on(store.get(&k, range)),
        Err(Error::ByteRangeOutOfBounds {
            key: k.clone(),
            offset: 10,
            length: 5,
            object_size: 3,
        }),
        "start-past-size is deterministic and never retryable, on every backend"
    );
}

fn conditional_write_if_absent(store: &dyn ObjectStore) {
    let k = key("conformance/if_absent.seg");
    // ⚠️ **Establishes its own precondition instead of assuming a virgin
    // backend — found by running this suite against MinIO a second time.**
    // Every other case either opens with an unconditional `put` (which
    // overwrites whatever a previous run left), deletes what it created, or
    // writes nothing at all, so the suite as a whole looked idempotent. This one asserts a key is
    // *absent* and then makes it present, so run two against a persistent
    // backend failed where run one passed — and run one only passed because
    // the bucket happened to be empty. A suite that is green exactly once per
    // bucket cannot be what a milestone's completion condition reads.
    //
    // Deleting at the *start* rather than cleaning up at the end is the part
    // that matters: a run that dies midway through this case leaves the
    // object behind either way, and only a delete-first recovers from that
    // without a human emptying the bucket.
    block_on(store.delete(std::slice::from_ref(&k)))
        .expect("clearing anything a previous run left behind succeeds");
    block_on(store.put(&k, vec![1], Some(Precondition::IfAbsent)))
        .expect("if-absent succeeds against an absent key");
    let result = block_on(store.put(&k, vec![2], Some(Precondition::IfAbsent)));
    assert!(
        matches!(result, Err(Error::PreconditionFailed { .. })),
        "if-absent must fail once the key exists, got {result:?}"
    );
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Ok(vec![1]),
        "the losing conditional put must not have touched the object"
    );
}

fn conditional_write_if_matches_succeeds_with_the_current_token(store: &dyn ObjectStore) {
    let k = key("conformance/if_matches_current.seg");
    let first = block_on(store.put(&k, vec![1], None)).expect("first put");

    block_on(store.put(
        &k,
        vec![2],
        Some(Precondition::IfMatches(first.precondition_token)),
    ))
    .expect("if-matches must succeed when presented the key's actual current token");

    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(vec![2]));
}

fn conditional_write_if_matches_rejects_stale_token(store: &dyn ObjectStore) {
    let k = key("conformance/if_matches_stale.seg");
    let stale = block_on(store.put(&k, vec![1], None)).expect("first put");
    block_on(store.put(&k, vec![2], None)).expect("an intervening overwrite");

    let result = block_on(store.put(
        &k,
        vec![3],
        Some(Precondition::IfMatches(stale.precondition_token)),
    ));
    assert!(
        matches!(result, Err(Error::PreconditionFailed { .. })),
        "if-matches must fail against a token from before an intervening overwrite, got {result:?}"
    );
}

fn ranged_get_reads_exactly_the_requested_slice(store: &dyn ObjectStore) {
    let k = key("conformance/ranged_get.seg");
    block_on(store.put(&k, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9], None)).expect("put succeeds");
    let range = ByteRange::bounded(2, 3).expect("a valid range");
    assert_eq!(block_on(store.get(&k, range)), Ok(vec![2, 3, 4]));
}
