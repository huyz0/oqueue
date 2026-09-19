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

mod listing;
pub mod record;

use oqueue_core::{ByteRange, Error, MaintenanceStore, ObjectKey, ObjectStore, Precondition};
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

/// What a case is handed: the store, and whatever this backend can be told to
/// do to itself.
///
/// ⚠️ **A fault a case needs is a *capability*, not a knob it reaches for.**
/// `crash_after_put_before_ack` lives on `oqueue-core`'s fake and nowhere
/// else, and a suite that downcast to reach it would be a suite that only ever
/// tested the fake. So the backend supplies the trigger or declares it cannot,
/// and the case is skipped and recorded — never silently passed.
///
/// ⚠️ **One trigger, and the shape is deliberately not general.** A registry
/// of named faults would be a seam with one caller and room to rot; a field
/// per fault makes adding the second one a visible edit here.
pub struct Harness<'a> {
    store: &'a dyn ObjectStore,
    /// The same backend, seen through the listing seam (`M7.3a`,
    /// `ADR-0009` §2): every backend the suite runs implements both.
    lister: &'a dyn MaintenanceStore,
    arm_crash_after_put: Option<&'a dyn Fn()>,
}

// ⚠️ Hand-written: `&dyn Fn()` has no `Debug`, and a derive would refuse. What
// a report needs to say is which cases ran, not what the trigger was.
impl core::fmt::Debug for Harness<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Harness")
            .field("can_crash_after_put", &self.arm_crash_after_put.is_some())
            .finish()
    }
}

impl<'a> Harness<'a> {
    /// A harness over `store` that can inject no faults.
    #[must_use]
    pub const fn new<S: ObjectStore + MaintenanceStore>(store: &'a S) -> Self {
        Self {
            store,
            lister: store,
            arm_crash_after_put: None,
        }
    }

    /// The same, able to arm one durable-write-then-fail on demand.
    ///
    /// A backend that supplies this must also declare
    /// [`Capabilities::injectable_ack_loss`]; the two are separate because
    /// declaring the capability is what makes an *absent* trigger a loud
    /// failure rather than a quietly skipped case.
    #[must_use]
    pub const fn with_crash_after_put(mut self, arm: &'a dyn Fn()) -> Self {
        self.arm_crash_after_put = Some(arm);
        self
    }

    /// The store under test.
    #[must_use]
    pub const fn store(&self) -> &dyn ObjectStore {
        self.store
    }

    /// The store under test, through its listing seam.
    #[must_use]
    pub const fn lister(&self) -> &dyn MaintenanceStore {
        self.lister
    }

    /// Arms the next `put` to write durably and then answer an error.
    ///
    /// # Panics
    ///
    /// If this backend declared [`Capabilities::injectable_ack_loss`] and then
    /// supplied no trigger. ⚠️ **A panic, not a skip**: the case only runs
    /// because the capability was declared, so a missing trigger is a lie in
    /// the declaration and must fail loudly.
    pub fn arm_crash_after_put(&self) {
        (self
            .arm_crash_after_put
            .expect("a backend declaring injectable_ack_loss must supply a trigger"))();
    }
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
    /// The backend can be made to lose a `put`'s acknowledgement *after* the
    /// bytes are durable — the "unknown state" of `ADR-0005` **guarantee 2**.
    ///
    /// ⚠️ **A harness capability, not a durability property**, and the
    /// distinction is worth the sentence: `S3Store` declaring this `false`
    /// says nothing can *tell* S3 to drop an acknowledgement, not that S3 is
    /// less durable than a fake. Read the other way round it inverts the
    /// truth, since S3 is the backend that actually provides guarantee 1 and
    /// the fake is the one that cannot.
    ///
    /// ⚠️ **And it is not guarantee 1's crash clause.** That one — `Ok`
    /// implies durable *across a process crash* — no single-process fake can
    /// demonstrate, which `ADR-0005`'s own Consequences say in as many words;
    /// `M1.44` deferred it to `M15`'s real-backend verification and this flag
    /// does not touch it. What `M1.37` found missing and this supplies is the
    /// flag a durability-shaped case can be gated on, and one such case.
    pub injectable_ack_loss: bool,
}

impl Capabilities {
    /// Every capability this suite currently knows to ask about.
    pub const FULL: Self = Self {
        conditional_writes: true,
        ranged_reads: true,
        injectable_ack_loss: true,
    };
}

/// One case in the suite: a name, what capability (if any) it needs, and the
/// assertion itself.
struct Case {
    name: &'static str,
    requires: fn(Capabilities) -> bool,
    run: fn(&Harness),
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
fn c(name: &'static str, requires: fn(Capabilities) -> bool, run: fn(&Harness)) -> Case {
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
        c(
            "a_failed_put_is_not_proof_of_absence",
            |c| c.injectable_ack_loss,
            a_failed_put_is_not_proof_of_absence,
        ),
    ]
}

/// Runs every case against `store`, skipping (and recording, in the
/// returned report) any case `capabilities` declares unsupported.
#[must_use]
pub fn run_conformance_suite(
    backend_name: &'static str,
    harness: &Harness,
    capabilities: Capabilities,
) -> ConformanceReport {
    let mut ran = Vec::new();
    let mut skipped = Vec::new();
    for case in cases().into_iter().chain(listing::cases()) {
        if (case.requires)(capabilities) {
            (case.run)(harness);
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

fn put_get_roundtrip(harness: &Harness) {
    let store = harness.store();
    let k = key("conformance/put_get_roundtrip.seg");
    let payload = vec![0x00, 0xff, 0x42, 0x00, 0x7f];
    block_on(store.put(&k, payload.clone(), None)).expect("put succeeds");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(payload));
}

fn get_of_never_put_key_is_not_found(harness: &Harness) {
    let store = harness.store();
    let k = key("conformance/never_put.seg");
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Err(Error::ObjectNotFound { key: k })
    );
}

fn empty_object_is_distinct_from_missing(harness: &Harness) {
    let store = harness.store();
    let k = key("conformance/empty.seg");
    block_on(store.put(&k, Vec::new(), None)).expect("put succeeds");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(Vec::new()));
}

fn delete_is_idempotent(harness: &Harness) {
    let store = harness.store();
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
fn conditional_write_above_the_chunk_preference_is_one_request(harness: &Harness) {
    let store = harness.store();
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
fn ranged_get_starting_past_the_object_size_is_out_of_bounds(harness: &Harness) {
    let store = harness.store();
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

fn conditional_write_if_absent(harness: &Harness) {
    let store = harness.store();
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

fn conditional_write_if_matches_succeeds_with_the_current_token(harness: &Harness) {
    let store = harness.store();
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

fn conditional_write_if_matches_rejects_stale_token(harness: &Harness) {
    let store = harness.store();
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

fn ranged_get_reads_exactly_the_requested_slice(harness: &Harness) {
    let store = harness.store();
    let k = key("conformance/ranged_get.seg");
    block_on(store.put(&k, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9], None)).expect("put succeeds");
    let range = ByteRange::bounded(2, 3).expect("a valid range");
    assert_eq!(block_on(store.get(&k, range)), Ok(vec![2, 3, 4]));
}

/// ⚠️ **`ADR-0005` guarantee 2: a failed `put` is not proof of absence.**
///
/// The bytes may be durable even though the caller was told they are not,
/// because the failure can land between the write and its acknowledgement. A caller that read an error as "the object is not
/// there" would retry into a different key, or conclude a record was lost that
/// is sitting in the bucket, or delete around an object it does not know
/// exists.
///
/// ⚠️ **Guarantee 1 is a different clause and this does not test it.** "`Ok`
/// implies durable across a process crash" cannot be shown by any
/// single-process fake — `ADR-0005`'s Consequences say so — and `M1.44`
/// deferred it to `M15`'s real-backend verification. What this case is is the
/// *injectable* half: a failure landing after the write, which only a fake can
/// produce and which is therefore the one durability-shaped thing the suite
/// can check at all.
///
/// ⚠️ **What FR-10 requires is the other direction**: no *acknowledged* record
/// may be lost. Here nothing was acknowledged, so nothing was promised — and
/// the object being present anyway is exactly why "not acknowledged" may never
/// be read as "not written". The broker-side half, that a produce whose flush
/// failed is refused rather than given an offset, is `oqueue-broker`'s
/// `a_produce_whose_put_failed_is_refused_with_no_offset`.
///
/// ⚠️ **The fake's own behaviour is already pinned elsewhere**, in
/// `oqueue-core`'s `crash_after_put_writes_the_object_but_reports_failure`,
/// and this case does not replace it. What is new here is that the *suite*
/// asks every backend about it, so a backend that cannot answer says so in a
/// report instead of being quietly not asked — which is the whole of what
/// `M1.37` found missing.
///
/// ⚠️ **And the bytes are whole.** A half-written object would be the worse
/// failure: a caller retrying would overwrite it, but anything reading first
/// would see a truncated payload that no length check anywhere would catch.
fn a_failed_put_is_not_proof_of_absence(harness: &Harness) {
    let store = harness.store();
    let k = key("conformance/failed_put_is_not_absence.seg");
    let payload: Vec<u8> = (0..64_u8).collect();

    harness.arm_crash_after_put();
    let outcome = block_on(store.put(&k, payload.clone(), None));

    assert!(
        outcome.is_err(),
        "the trigger must make this put fail; it is what the case is about"
    );
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Ok(payload),
        "a put that failed after the write must leave the whole object behind — \
         reading its error as absence is how an acknowledged record gets lost"
    );

    // And the retry a caller makes next is safe, because it rewrites its own
    // key with its own bytes — which is what `ADR-0026` rests on.
    let again: Vec<u8> = (64..128_u8).collect();
    block_on(store.put(&k, again.clone(), None)).expect("the retry succeeds");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(again));
}
