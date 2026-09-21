use super::{handle, seat};
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::SyncGroupRequest as KpRequest;
use kafka_protocol::messages::SyncGroupResponse as KpResponse;
use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment as KpAssignment;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;

/// `protocol_type`/`protocol_name` are absent below v5 and echoed from v5
/// on (`oqueue_codec::sync_group`'s own doc) — this test is the only one in
/// this file at v5+, and exists specifically so that cutover is exercised
/// on the wire at all, not only in `response_for`'s own in-memory value.
#[tokio::test(start_paused = true)]
async fn protocol_type_and_name_are_echoed_on_the_wire_from_v5() {
    const V5: i16 = 5;
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1"]);
    let mut assignment = KpAssignment::default();
    assignment.member_id = StrBytes::from_static_str("m1");
    assignment.assignment = bytes::Bytes::from_static(b"a");
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("orders-consumers"),
        ))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_static_str("m1"))
        .with_protocol_type(Some(StrBytes::from_static_str("consumer")))
        .with_protocol_name(Some(StrBytes::from_static_str("range")))
        .with_assignments(vec![assignment]);
    let mut body = Vec::new();
    request.encode(&mut body, V5).expect("encodes");

    let prelude = RequestPrelude {
        api_key: 14,
        api_version: V5,
        correlation_id: 9,
    };
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        prelude,
        &body,
        &crate::authz::unconfigured_group_authz(),
    )
    .await
    else {
        panic!("a SyncGroup replies");
    };
    let mut rest = &out[5..]; // v5 is flexible: a 5-byte response header.
    let response = KpResponse::decode(&mut rest, V5).expect("decodes");
    assert!(rest.is_empty());
    assert_eq!(
        response.protocol_type.as_ref().map(StrBytes::as_str),
        Some("consumer")
    );
    assert_eq!(
        response.protocol_name.as_ref().map(StrBytes::as_str),
        Some("range")
    );
}
