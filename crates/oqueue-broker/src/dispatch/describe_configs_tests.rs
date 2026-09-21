#![allow(clippy::expect_used)]

use super::{dispatcher, replied};
use kafka_protocol::messages::DescribeConfigsResponse;
use kafka_protocol::protocol::Decodable;
use oqueue_codec::flex::{put_array_len, put_string};
use oqueue_codec::wire::{Cursor, put_bool, put_i8, put_i16, put_i32};

#[tokio::test]
async fn reports_the_effective_topic_retention_default() {
    let (dispatcher, fixture) = dispatcher().await;
    fixture
        .cluster
        .create_topic(&oqueue_core::TopicId::new("orders").expect("topic"), 1)
        .await
        .expect("topic");

    let mut body = Vec::new();
    put_i32(&mut body, 1);
    put_i8(&mut body, 2);
    put_string(&mut body, false, "orders");
    put_array_len(&mut body, false, None);
    put_bool(&mut body, false);
    let mut request = Vec::new();
    put_i16(&mut request, 32);
    put_i16(&mut request, 1);
    put_i32(&mut request, 88);
    put_i16(&mut request, -1);
    request.extend_from_slice(&body);

    let out = replied(&dispatcher, request).await;
    let mut correlation = Cursor::new(&out);
    assert_eq!(correlation.read_i32().expect("correlation"), 88);
    let mut response_bytes = &out[4..];
    let response = DescribeConfigsResponse::decode(&mut response_bytes, 1).expect("response");
    assert!(response_bytes.is_empty());
    assert_eq!(response.results[0].error_code, 0);
    assert_eq!(response.results[0].configs[0].name.as_str(), "retention.ms");
    assert_eq!(
        response.results[0].configs[0].value.as_deref(),
        Some("604800000")
    );
}
