use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_codec::apikey::ApiKey;

pub(super) fn request_body(out: &mut Vec<u8>, api_key: ApiKey, version: i16) {
    match api_key {
        ApiKey::DescribeClientQuotas => {
            kafka_protocol::messages::DescribeClientQuotasRequest::default()
                .encode(out, version)
                .expect("encodes");
        }
        ApiKey::AlterClientQuotas => {
            kafka_protocol::messages::AlterClientQuotasRequest::default()
                .encode(out, version)
                .expect("encodes");
        }
        other => panic!("not a quota API: {other:?}"),
    }
}

pub(super) fn error_code(api_key: ApiKey, rest: &mut &[u8], version: i16) -> i16 {
    match api_key {
        ApiKey::DescribeClientQuotas => {
            kafka_protocol::messages::DescribeClientQuotasResponse::decode(rest, version)
                .expect("DescribeClientQuotas reply decodes")
                .error_code
        }
        ApiKey::AlterClientQuotas => {
            kafka_protocol::messages::AlterClientQuotasResponse::decode(rest, version)
                .expect("AlterClientQuotas reply decodes")
                .entries
                .first()
                .map_or(0, |entry| entry.error_code)
        }
        other => panic!("not a quota API: {other:?}"),
    }
}
