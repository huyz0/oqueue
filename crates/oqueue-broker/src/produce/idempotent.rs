//! `M11.4`-`M11.7`'s end-to-end tests: idempotent producers, on the wire.
//!
//! ⚠️ **Split from `tests.rs` at the 500-line limit, along the seam** —
//! that file is `Produce`'s ordinary, pre-`M11` behavior; this is the
//! idempotent-producer surface built on top of it, exercised through the
//! same `handle`/`replied`/`verdict` helpers `tests.rs` and
//! `answer.rs`'s own suites already share.

#![allow(clippy::expect_used)]

use super::tests::{body_for, produce_body, replied, verdict};
use crate::testing::{fixture, golden_batch, partition, topic};
use kafka_protocol::messages::TopicName;
use kafka_protocol::messages::produce_request::TopicProduceData;
use kafka_protocol::protocol::StrBytes;

/// A one-record batch under a real idempotent-producer identity —
/// `golden_batch_of`'s own precedent (`crate::testing`), with the producer
/// fields `M11.5` threads through instead of left at `-1`.
fn idempotent_batch(producer_id: i64, epoch: i16, sequence: i32) -> Vec<u8> {
    use kafka_protocol::records::{
        Compression, Record, RecordBatchEncoder, RecordEncodeOptions, TimestampType,
    };
    let record = Record {
        transactional: false,
        control: false,
        partition_leader_epoch: 0,
        producer_id,
        producer_epoch: epoch,
        timestamp_type: TimestampType::Creation,
        offset: 0,
        sequence,
        delete_horizon: false,
        timestamp: 1_700_000_000_000,
        key: None,
        value: Some(bytes::Bytes::from_static(b"hello")),
        headers: kafka_protocol::indexmap::IndexMap::default(),
    };
    let mut buf = bytes::BytesMut::new();
    RecordBatchEncoder::encode(
        &mut buf,
        std::slice::from_ref(&record),
        &RecordEncodeOptions {
            version: 2,
            compression: Compression::None,
        },
    )
    .expect("the dependency encodes its own records");
    buf.to_vec()
}

/// ⚠️ **`M11.6`, end to end.** A retried batch — same producer, same epoch,
/// same sequence — is answered as an ordinary success naming the offset the
/// *first* send occupied, never a second one and never a refusal.
/// `ADR-0031` point 2's transparent-success case, on the wire.
#[tokio::test]
async fn a_retried_sequence_is_answered_success_with_the_first_offset() {
    let fixture = fixture(&["t"]).await;
    let batch = idempotent_batch(7, 0, 0);
    let body = produce_body(9, "t", -1, batch.clone());

    let first = verdict(&replied(&fixture, 9, &body).await);
    assert_eq!(first.0, 0, "the first send is an ordinary success");

    let retry = verdict(&replied(&fixture, 9, &body).await);
    assert_eq!(
        retry, first,
        "the same sequence, answered the same way, both times"
    );
    assert_eq!(
        fixture
            .cluster
            .high_watermark(&topic("t"), partition(0))
            .get(),
        1,
        "the retry never advanced the log"
    );
}

/// A producer's first-ever span at a nonzero sequence is a gap — refused
/// with the code a real client's retry logic already knows, not the
/// generic refusal every other unrelated failure gets.
#[tokio::test]
async fn a_sequence_gap_is_refused_with_the_out_of_order_wire_code() {
    let fixture = fixture(&["t"]).await;
    let body = produce_body(9, "t", -1, idempotent_batch(7, 0, 5));

    assert_eq!(
        verdict(&replied(&fixture, 9, &body).await),
        (
            kafka_protocol::error::ResponseError::OutOfOrderSequenceNumber.code(),
            -1
        )
    );
    assert_eq!(
        fixture
            .cluster
            .high_watermark(&topic("t"), partition(0))
            .get(),
        0,
        "a rejected span never reaches the log"
    );
}

/// ⚠️ **The finding review caught before this shipped.** An all-rejected
/// commit can leave `CoordinatorLoop::serve` with nothing ever published —
/// there is no version for a fresh coordinator's `last_committed` fallback
/// to name that is actually true — so observing it regardless would raise
/// this session's read-your-writes bar to a version `IndexWatch`'s own
/// `AtLeast` wait can never see satisfied, stalling every later fetch on
/// this connection for a write that never happened. A produce answered
/// entirely in refusals must raise no bar at all.
#[tokio::test]
async fn an_all_rejected_produce_leaves_the_sessions_watermark_untouched() {
    let fixture = fixture(&["t"]).await;
    assert_eq!(fixture.session.watermark(), None, "nothing produced yet");

    let body = produce_body(9, "t", -1, idempotent_batch(7, 0, 5));
    assert_eq!(
        verdict(&replied(&fixture, 9, &body).await).0,
        kafka_protocol::error::ResponseError::OutOfOrderSequenceNumber.code()
    );

    assert_eq!(
        fixture.session.watermark(),
        None,
        "a produce that wrote nothing raises no read-your-writes bar"
    );
}

/// ⚠️ **The property this milestone exists to deliver, on the actual wire
/// path**: one producer's rejected gap must not cost a different topic's
/// legitimate span, in the very same `Produce` request, its real offset.
#[tokio::test]
async fn one_topics_rejection_does_not_cost_anothers_offset_in_one_request() {
    let fixture = fixture(&["a", "b"]).await;
    let mut t_a = TopicProduceData::default();
    t_a.name = TopicName(StrBytes::from_string("a".to_owned()));
    let mut t_b = TopicProduceData::default();
    t_b.name = TopicName(StrBytes::from_string("b".to_owned()));
    let body = body_for(
        9,
        vec![
            (t_a, idempotent_batch(7, 0, 5)), // a gap: rejected
            (t_b, golden_batch()),            // ordinary produce: admitted
        ],
        -1,
    );

    let response = replied(&fixture, 9, &body).await;
    let a = &response.responses[0].partition_responses[0];
    let b = &response.responses[1].partition_responses[0];
    assert_eq!(
        a.error_code,
        kafka_protocol::error::ResponseError::OutOfOrderSequenceNumber.code()
    );
    assert_eq!(a.base_offset, -1);
    assert_eq!(b.error_code, 0, "an unrelated topic's produce still lands");
    assert_eq!(b.base_offset, 0);
}

/// ⚠️ **`M11.7`, end to end.** A zombie — an epoch older than one this
/// broker has already seen for the same producer id — is refused with the
/// code real Kafka reserves for exactly this (`INVALID_PRODUCER_EPOCH`),
/// not the generic `OUT_OF_ORDER_SEQUENCE_NUMBER` an ordinary gap gets.
#[tokio::test]
async fn a_zombie_epoch_is_refused_with_the_invalid_producer_epoch_wire_code() {
    let fixture = fixture(&["t"]).await;

    // Some other incarnation of producer 7 already bumped to epoch 1.
    let bumped = produce_body(9, "t", -1, idempotent_batch(7, 1, 0));
    assert_eq!(verdict(&replied(&fixture, 9, &bumped).await).0, 0);

    // This sender is still at epoch 0 — a zombie.
    let zombie = produce_body(9, "t", -1, idempotent_batch(7, 0, 1));
    assert_eq!(
        verdict(&replied(&fixture, 9, &zombie).await),
        (
            kafka_protocol::error::ResponseError::InvalidProducerEpoch.code(),
            -1
        )
    );
    assert_eq!(
        fixture
            .cluster
            .high_watermark(&topic("t"), partition(0))
            .get(),
        1,
        "the zombie's own commit never reached the log"
    );
}
