//! Two produces that genuinely overlap one parked fetch, split from
//! `invariants.rs` at the 500-line limit (`M10.32`).
//!
//! ⚠️ **`RacedProduce` never reaches a genuine choice, measured directly.**
//! `a_parked_fetch_races_a_commit`'s own doc records what changed: one
//! serialized produce always commits before `park.rs`'s deadline can elapse,
//! so the commit wins every time and the `select!` never has two ready arms
//! to choose between. Two produces racing the *same* park — spawned, not
//! `.await`ed in sequence — is what the row after it (`M10.12`) named as
//! what would close the gap: which commit the coordinator and the watch see
//! first is now a genuine question of how the two spawned tasks interleave,
//! not a foregone one decided by the calling task running one to completion
//! before the other is even polled.
//!
//! ⚠️ **What this closes, and what it does not.** This makes the two
//! produces overlap rather than serialize, which `Step::RacedProduce` did
//! not and could not — that much is structural, not measured. Whether
//! `park.rs`'s *own* two `select!` arms ever land in the same poll as a
//! result is a separate, narrower claim this file does not make: every
//! `FaultConfig` knob is deliberately unable to consume virtual time
//! (`fault.rs`'s own doc), so nothing here can make a commit's landing and
//! the deadline's elapse coincide on the clock. What is genuinely new is the
//! *task* interleaving — which of two spawned produces the coordinator
//! processes first — and that is exactly `M10.32`'s own text: "which commit
//! the watch sees first is a real question".

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]
#![allow(clippy::redundant_pub_crate)]

use crate::invariants::{PARK_MS, Run, observe, starts_at_the_beginning};
use crate::roundtrip::{fetch_response_of, parked_fetch_frame, produce_frame};
use crate::support::Broker;
use kafka_protocol::messages::ProduceResponse;
use kafka_protocol::protocol::Decodable as _;
use oqueue_broker::{Dispatcher, Handler as _, HandlerResponse};
use std::sync::Arc;

/// Decodes a `Produce` reply frame, the write-path counterpart of
/// `roundtrip::fetch_response_of`.
fn produce_response_of(reply: &[u8]) -> ProduceResponse {
    let mut rest = crate::roundtrip::body_of(reply, true);
    let response =
        ProduceResponse::decode(&mut rest, crate::roundtrip::PRODUCE_VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// Parks a fetch, then races **two** produces against it — spawned together
/// rather than one `run.step` serializing under the park, which is the
/// difference `a_parked_fetch_races_a_commit` cannot make on its own.
///
/// ⚠️ **Both frames built before either is spawned.** `produce_frame` takes
/// `&Broker`, so building both up front and moving only owned bytes into each
/// spawned task is what lets the two run as independent tasks at all — a
/// `Broker` reference cannot cross into a `'static` spawn.
pub async fn two_produces_race_one_parked_fetch(broker: &Broker, run: &mut Run) {
    run.races += 1;
    let at = i64::from(u32::try_from(run.acked.map_or(0, |last| last + 1)).unwrap_or(0));
    let parked_frame = parked_fetch_frame(broker, "orders", at, PARK_MS);
    let parked = {
        let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
        tokio::spawn(async move { dispatcher.handle(parked_frame).await })
    };

    let frame_a = produce_frame(broker, "orders");
    let frame_b = produce_frame(broker, "orders");
    let dispatcher_a = Dispatcher::new(Arc::clone(&broker.cluster));
    let dispatcher_b = Dispatcher::new(Arc::clone(&broker.cluster));
    let task_a = tokio::spawn(async move { dispatcher_a.handle(frame_a).await });
    let task_b = tokio::spawn(async move { dispatcher_b.handle(frame_b).await });

    // ⚠️ **Awaited in a fixed order, and that pins nothing about which one
    // actually landed first.** `JoinHandle::await` resolves once its own
    // task has finished; both tasks were already spawned and running
    // concurrently, so awaiting `task_a` before `task_b` says nothing about
    // which one the coordinator processed first — that question was settled
    // by the two tasks' own interleaving, not by this order.
    let HandlerResponse::Reply(reply_a) = task_a.await.expect("produce a's task lives") else {
        panic!("a produce answers")
    };
    let HandlerResponse::Reply(reply_b) = task_b.await.expect("produce b's task lives") else {
        panic!("a produce answers")
    };
    let response_a = produce_response_of(&reply_a);
    let response_b = produce_response_of(&reply_b);
    run.absorb(&response_a.responses[0].partition_responses[0]);
    run.absorb(&response_b.responses[0].partition_responses[0]);

    if let Some(observation) = observe(&run.dispatcher, broker, run.orphans).await {
        starts_at_the_beginning(&observation);
        run.invariants
            .check(&observation)
            .unwrap_or_else(|broken| panic!("{broken}"));
    }

    let HandlerResponse::Reply(reply) = parked.await.expect("the parked task lives") else {
        panic!("a fetch answers")
    };
    let response = fetch_response_of(&reply);
    let partition = &response.responses[0].partitions[0];
    assert_eq!(
        partition.error_code, 0,
        "a parked fetch that loses its race answers empty, never an error"
    );
}
