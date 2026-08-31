//! The request pipeline: decode the key, refuse an unmodelled method, apply a
//! fault, dispatch, withhold the answer.
//!
//! ⚠️ **This file has been split three times in two rows and its own summary
//! was wrong after two of them**, which is the argument for saying plainly
//! what is left rather than what was here: the wire is `transport.rs`, the data
//! is `state.rs`, and what each method *does* is `handlers.rs`. The pipeline
//! grows with the harness; the handlers grow with S3.
//!
//! ⚠️ **The surviving-mutant inventory moved with the code it is about.** It
//! was here, naming branches that are all now in `handlers.rs`, which is where
//! an editor meets them and where the warning has to be to do anything. What
//! stays here is the pipeline's own: `M10.7`'s three interception points and
//! `M10.8`'s fourth were hand-mutated and are guarded — the storm's
//! `remaining > 0`, the refusal branch, the `lost_race` plant position, and the
//! pause's placement after the dispatch — ⚠️ **except the two faults' `method`
//! discriminators**, of which only `Pause`'s is owned by a case; `Storm`'s can
//! be weakened to match any method with the suite green, and closing that is
//! `M10.7`'s debt rather than this row's.
//!
//! ⚠️ **`cargo mutants` generates nothing for `tests/`**, verified — so no gate
//! constrains any of this and a claim about what is guarded here is worth only
//! the measurement behind it.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]
// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the bare
// `pub` clippy's `redundant_pub_crate` asks for. The same trade the broker's
// `region.rs` and `oqueue-core`'s `page.rs` make, for the same reason.
#![allow(clippy::redundant_pub_crate)]

use super::encoding::percent_decode;
use super::faults::require_a_single_threaded_runtime;
use super::state::ModelState;
use super::transport::{body_bytes, status};
use std::sync::{Arc, Mutex};

use object_store::client::{HttpRequest, HttpResponse};

/// The bucket every request in this file is addressed to.
pub(crate) const BUCKET: &str = "oqueue-sim";

/// An in-process S3, deterministic and shared by clones.
///
/// ⚠️ **A `std::sync::Mutex` and not an async one, deliberately.** The one
/// `.await` in `respond` collects the request body *before* the lock is taken,
/// so the guard is never held across a suspension point and
/// `async-concurrency.md`'s rule against that is not in play.
#[derive(Debug, Default, Clone)]
pub(crate) struct ModelS3 {
    state: Arc<Mutex<ModelState>>,
}

impl ModelS3 {
    /// The key a request addresses, with the bucket segment removed.
    ///
    /// ⚠️ Path-style, because `AmazonS3Builder::with_virtual_hosted_style_request`
    /// is left off below — so a path is `/<bucket>/<key>` and never a
    /// bucket-qualified host.
    fn key_of(req: &HttpRequest) -> String {
        percent_decode(
            req.uri()
                .path()
                .trim_start_matches('/')
                .trim_start_matches(BUCKET)
                .trim_start_matches('/'),
        )
    }

    /// Refuses a request the model has no branch for, before any state is
    /// touched.
    ///
    /// ⚠️ **Before the lock is taken.** The panic used to fire while the guard
    /// was live, which poisons the mutex — so the next request through any
    /// clone died on an `expect`, asserting the opposite of what happened and
    /// naming neither method nor URI. That is the same displacement three
    /// layers from the gap that the panic replaced a 405 to remove.
    fn assert_modelled(req: &HttpRequest) {
        assert!(
            matches!(req.method().as_str(), "PUT" | "GET" | "HEAD" | "DELETE")
                || (req.method() == "POST" && req.uri().query() == Some("delete")),
            "the model has no branch for {} {} — add one rather than teaching a \
             test to expect a backend error",
            req.method(),
            req.uri()
        );
    }

    /// The answer a modelled request gets, once no fault has replaced it.
    ///
    /// ⚠️ **Its own function because `respond` outgrew fifty lines the moment
    /// faults were added**, which `code-structure.md` calls a design signal:
    /// what a request *means* and what the harness does *to* it are two
    /// concerns that grew at different rates.
    fn dispatch(
        state: &mut ModelState,
        req: &HttpRequest,
        key: String,
        body: Vec<u8>,
    ) -> HttpResponse {
        match req.method().as_str() {
            "PUT" => Self::put(state, req, key, body),
            "GET" => Self::get(state, req, &key),
            // ⚠️ **A HEAD is not optional for this model.** `oqueue-store`
            // answers a failed ranged GET by asking for the object's size and
            // deciding between `ByteRangeOutOfBounds` and a retry from it — so
            // a model that refused HEAD turned a 416 into eleven retries and a
            // `Transient`, which is what the first version did.
            "HEAD" => Self::head(state, &key),
            // ⚠️ **Reachable only through `object_store`'s single-object
            // path, which `S3Store` does not take** — its `delete` goes to the
            // bulk `POST ?delete` below. Kept because the method is part of
            // what an S3 answers, and marked because review found this arm
            // dead with a confident comment beside it, exactly as it found the
            // 416 branch dead one round earlier.
            "DELETE" => {
                state.objects.remove(&key);
                status(204)
            }
            // ⚠️ **This is how `S3Store` actually deletes.**
            // `ObjectStoreExt::delete` goes through `delete_stream`, which the
            // S3 client turns into one `POST /<bucket>?delete` carrying an XML
            // list — so a model that answered only `DELETE` removed nothing,
            // returned `Transient`, and would have failed three of the
            // conformance cases `M10.3` runs.
            "POST" if req.uri().query() == Some("delete") => Self::bulk_delete(state, &body),
            // ⚠️ **A panic, not a status.** Two earlier versions answered 501
            // and then 405, and both turned a *model gap* into
            // `Error::Transient` — a code the taxonomy defines as a retryable
            // transport blip — three layers from the missing branch, which is
            // how the bulk-delete gap above stayed invisible through a round of
            // review. A panic in a test harness is the test failing, and it
            // names the request that has no branch. It also never reaches
            // `object_store`'s retry layer, which is what the 405 bought.
            method => panic!(
                "the model has no branch for {method} {} — add one rather than \
             teaching a test to expect a backend error",
                req.uri()
            ),
        }
    }

    pub(crate) async fn respond(&self, req: HttpRequest) -> HttpResponse {
        // ⚠️ **Collected before the lock is taken**, which is what keeps the
        // guard off an `await` — `async-concurrency.md`'s rule. Every method's
        // body is read, not only a PUT's: a GET's is empty and collecting it
        // costs nothing, where a `match` that collected only sometimes would
        // be a second place for the method dispatch to disagree with itself.
        let (req, body) = body_bytes(req).await;
        let key = Self::key_of(&req);
        Self::assert_modelled(&req);
        // ⚠️ **Drawn under the lock, slept outside it**, because a guard held
        // across an `.await` is what `async-concurrency.md` forbids. The lock
        // is not optional in any case: `Latency::draw` takes `&mut self`.
        //
        // ⚠️ **It does not buy determinism, and a first version said it did.**
        // A mutex serializes access without ordering it, so two concurrent
        // requests on a multi-thread runtime take the draws in whichever order
        // the workers arrive — one seed, two runs. What buys determinism is the
        // `current_thread` runtime `ADR-0028` specifies, and `M10.7` made that
        // a refusal rather than a caveat: `with_latency` and `inject` both
        // assert the flavour, so the conformance run — `multi_thread` because
        // `conformance::block_on` spins — cannot acquire either by accident.
        let delay = {
            let mut state = self
                .state
                .lock()
                .expect("the model's lock is never poisoned");
            state.latency.as_mut().map(|l| {
                l.draw(if matches!(req.method().as_str(), "PUT" | "POST") {
                    super::latency::Op::Write
                } else {
                    super::latency::Op::Read
                })
            })
        };
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }

        // ⚠️ **A block, not an explicit `drop`.** The guard has to be out of
        // *scope* before the pause below is awaited, not merely dropped:
        // `rustc` records a `MutexGuard` still in scope at an await point in
        // the future's own layout, so the whole `HttpService` future stops
        // being `Send` and the impl no longer compiles. Which is
        // `async-concurrency.md`'s rule enforced by the type system rather
        // than by a comment.
        let (resp, held) = {
            let mut state = self
                .state
                .lock()
                .expect("the model's lock is never poisoned");
            // ⚠️ **After the delay and before the dispatch.** A shedding S3 still
            // takes time to say so, so a storm composes with `latency` rather than
            // short-circuiting it; and answering before the `match` is what keeps
            // a stormed request from also *doing* the thing it refused.
            if let Some(code) = state.faults.storm(req.method().as_str()) {
                return status(code);
            }
            // ⚠️ Matched as a string so this file names no `http` crate — the same
            // discipline `transport.rs`'s `status()` follows, and the reason
            // neither needs a dependency `object_store` already carries.
            let resp = Self::dispatch(&mut state, &req, key, body);
            // ⚠️ **Decided under the lock, waited outside it**, exactly as the
            // latency draw above is, and for the same reason.
            let held = state.faults.pause(req.method().as_str());
            // ⚠️ Explicit and *inside* the block, which is not redundant with
            // the block: `significant_drop_tightening` wants the guard gone
            // before the tuple is built, and the block is what keeps it out of
            // the future's layout at the await below. Two different rules.
            drop(state);
            (resp, held)
        };
        // ⚠️ **After `resp` is built, which is what makes it a pause rather
        // than a slow request.** The state change has already happened; only
        // the answer is withheld. So a caller that gives up first gave up on
        // something that *did* occur — and if its future is dropped here, this
        // one is dropped with it and the mutation stands. A pause placed
        // before the dispatch would be cancellation instead, which is the
        // failure a kill already models.
        if let Some(d) = held {
            tokio::time::sleep(d).await;
        }
        resp
    }

    /// Answers every request after a delay drawn from doc 04 §2's curves.
    ///
    /// ⚠️ **Costs no real time only under a paused runtime.** Its callers write
    /// `#[tokio::test(start_paused = true)]`; on a plain `#[tokio::test]` every
    /// draw becomes a real `sleep`, which across a suite's requests would be
    /// tens of seconds against NFR-56's budget. ⚠️ **The flavour is asserted
    /// and the pause is not**, because the two are not equally checkable: a
    /// `multi_thread` run is wrong in a way no test can see, while a run that
    /// forgot `start_paused` is merely slow and says so in the timing.
    /// ⚠️ **Not `ADR-0028`'s runtime**, which is
    /// `oqueue-testkit::seeded_runtime` and adds a seeded scheduler this crate
    /// does not depend on — so a run here is paused but not seeded.
    #[must_use]
    pub(crate) fn with_latency(self, seed: u64) -> Self {
        require_a_single_threaded_runtime("simulated latency");
        self.state
            .lock()
            .expect("the model's lock is never poisoned")
            .latency = Some(super::latency::Latency::new(seed));
        self
    }

    /// Arms one fault, to be met by whichever request meets it.
    ///
    /// ⚠️ **Guarded like `with_latency`**, and for the same reason: a storm
    /// counted down by two workers in arrival order is a count no seed pins.
    pub(crate) fn inject(&self, fault: super::faults::Fault) {
        require_a_single_threaded_runtime("fault injection");
        self.state
            .lock()
            .expect("the model's lock is never poisoned")
            .faults
            .arm(fault);
    }

    /// How many requests this model has answered with an injected fault.
    pub(crate) fn faults_injected(&self) -> u32 {
        self.state
            .lock()
            .expect("the model's lock is never poisoned")
            .faults
            .injected()
    }

    /// Arms the one-shot above. Cheap enough to be called per case.
    ///
    /// ⚠️ **The third fault entry point, and the one that is deliberately
    /// *not* guarded** — `inject` and `with_latency` both refuse a
    /// multi-thread runtime and this does not, because
    /// `the_simulated_backend_passes_the_full_conformance_suite` arms it and
    /// is `multi_thread` by necessity. ⚠️ **What makes that safe is not stated
    /// anywhere else and is not a property of this method**:
    /// `conformance::block_on` drives one case at a time with a spin loop, so
    /// exactly one request is ever in flight and there is no arrival order for
    /// two workers to disagree about. A case that armed an ack loss and then
    /// issued two concurrent puts would get the hazard `require_a_single_threaded_runtime`
    /// exists to refuse, with nothing firing — so this is the exception that
    /// has to be argued rather than the rule.
    pub(crate) fn arm_ack_loss(&self) {
        self.state
            .lock()
            .expect("the model's lock is never poisoned")
            .lose_next_ack = true;
    }
}
