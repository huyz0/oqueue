#![allow(clippy::expect_used)]

use super::{
    ConfigOperation, IncrementalAlterConfigsResponse, IncrementalConfig,
    IncrementalConfigsResource, encode_response,
};
use crate::alter_configs::AlterConfigsResponseResource;
use crate::wire::DecodeError;

fn resource(configs: Vec<IncrementalConfig>) -> IncrementalConfigsResource {
    IncrementalConfigsResource {
        resource_type: 2,
        resource_name: "orders".to_owned(),
        configs,
    }
}

#[test]
fn operations_preserve_set_and_delete_semantics() {
    let set = resource(vec![IncrementalConfig {
        name: "retention.ms".to_owned(),
        operation: ConfigOperation::Set,
        value: Some("900000".to_owned()),
    }]);
    let delete = resource(vec![IncrementalConfig {
        name: "retention.ms".to_owned(),
        operation: ConfigOperation::Delete,
        value: None,
    }]);
    assert_eq!(set.configs[0].operation, ConfigOperation::Set);
    assert_eq!(delete.configs[0].operation, ConfigOperation::Delete);
}

#[test]
fn unknown_operation_is_rejected_by_the_decoder() {
    let mut body = vec![
        0, 0, 0, 1, 2, 0, 6, b'o', b'r', b'd', b'e', b'r', b's', 0, 0, 0, 1,
    ];
    body.extend_from_slice(&[0, 12]);
    body.extend_from_slice(b"retention.ms");
    body.push(9);
    body.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let error = super::decode_request(&body, 0).expect_err("unknown operation");
    assert!(matches!(
        error,
        DecodeError::LengthOutOfBounds { max: 3, .. }
    ));
}

#[test]
fn response_encoding_preserves_the_flexible_tag_section() {
    let response = IncrementalAlterConfigsResponse {
        throttle_time_ms: 7,
        resources: vec![AlterConfigsResponseResource {
            error_code: 0,
            error_message: Some("ok".to_owned()),
            resource_type: 2,
            resource_name: "orders".to_owned(),
        }],
    };
    let mut legacy = Vec::new();
    encode_response(&mut legacy, 0, &response);
    let mut flexible = Vec::new();
    encode_response(&mut flexible, 1, &response);
    assert!(flexible.len() < legacy.len());
    assert_eq!(&flexible[..4], &legacy[..4]);
}
