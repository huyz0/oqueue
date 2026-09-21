use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::heartbeat::encode_response;

pub(super) fn reply(prelude: RequestPrelude, version: i16, error_code: i16) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(&mut out, ApiKey::Heartbeat, version, prelude.correlation_id).is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, version, error_code);
    HandlerResponse::Reply(out)
}
