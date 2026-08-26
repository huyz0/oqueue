//! The whole request path, exactly as a socket delivers it: prelude,
//! `supports()` gate, every hand-rolled message decoder, and the handlers
//! over a live broker — `testing.md` rule 24's first-listed priority, and the
//! target whose first run found the `M2.26`/`M2.27` allocation `DoS`.
//!
//! ⚠️ **That `DoS` cannot recur here by construction now** (`ADR-0019`, `M2.34`):
//! no runtime code on this path imports `kafka-protocol`, and every hand-rolled
//! decoder bounds a count against the input before growing anything. No byte
//! string may panic or over-allocate; the worst legal outcome is `Close`.
//!
//! ⚠️ **A real cluster, not a stub** (`M3.14`). The handlers now seal bundles,
//! write objects and commit spans, so a stub would fuzz a path the broker no
//! longer runs. ⚠️ **And a fresh one per input**, which is not tidiness: a
//! shared cluster would carry one input's committed offsets into the next, and
//! a crash the fuzzer found would not reproduce from its own artifact.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::{Arc, OnceLock};

/// One runtime for the whole run — building one per input would measure
/// `Runtime::new` rather than the decoders.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            // ⚠️ **Timers, because a fetch parks** (`M3.20`). Without this
            // `sleep_until` panics, which this target reported as a crash the
            // first time the park existed — a harness fault rather than a
            // broker one, and exactly the sort a fuzz target is for.
            .enable_time()
            // ⚠️ **Paused, because `max_wait_ms` is fuzzer-controlled.** A
            // `Fetch` naming the maximum park would otherwise sleep a real
            // minute per input. Under paused time the deadline fires as soon
            // as nothing else can progress, so the park is *exercised* rather
            // than waited out.
            .start_paused(true)
            .build()
            .expect("a current-thread runtime")
    })
}

fuzz_target!(|data: &[u8]| {
    runtime().block_on(async {
        let log = Arc::new(oqueue_core::FakeMetadataLog::new());
        let index = Box::new(oqueue_core::FakeMaterializedIndex::new());
        let epoch = oqueue_core::CoordinatorEpoch::new(1);
        let (coordinator, serving, reader) =
            oqueue_coordinator::Coordinator::open(log, index, epoch)
                .await
                .expect("an empty log opens");
        let store: Arc<dyn oqueue_core::ObjectStore> = Arc::new(oqueue_core::FakeObjectStore::new());
        let cluster = oqueue_broker::Cluster::new(
            "fuzz",
            1,
            oqueue_broker::Sequencing::new(coordinator, reader),
            store,
            &oqueue_broker::WriterId::mint(),
        )
        .expect("a minted identity is a usable key component");
        cluster.ensure_topic("t");
        let dispatcher = oqueue_broker::Dispatcher::new(Arc::new(cluster));
        // ⚠️ **The coordinator loop runs `select!`ed, never `spawn`ed.** A
        // spawned task is aborted rather than joined, and on a current-thread
        // runtime the abort cannot complete before `block_on` returns — so one
        // dead task per input accumulated in the runtime and libFuzzer found
        // the OOM at around a hundred thousand runs. Running both futures in
        // this task means dropping the whole thing when the handler answers.
        //
        // The whole decode-and-answer path. Any outcome is legal; a panic, an
        // over-allocation or a hang is not.
        tokio::select! {
            _ = oqueue_broker::Handler::handle(&dispatcher, data.to_vec()) => {}
            () = serving.run() => {}
        }
    });
});
