//! `FindCoordinator` (10), single-key — `M4.3`, `ADR-0033`.
//!
//! ⚠️ **Every group resolves to this node, unconditionally** — `ADR-0033`'s
//! own decision, made real: v1 has one coordinator, this one, so there is
//! nothing to look up.
//!
//! ⚠️ **`key_type != 0` (`TRANSACTION`, from v1) is refused, not answered as
//! if understood** — `init_producer_id.rs`'s own precedent: `InitProducerId`
//! already refuses a `transactional_id` with `INVALID_REQUEST` because FR-15
//! (transactions) is deferred and nothing here backs the guarantee. Round 1
//! review found the first version of this handler answering a
//! `TRANSACTION`-type lookup with this node's own identity anyway —
//! `ADR-0033`'s "every group resolves to this node" is scoped to *groups*,
//! not transactions, and a client that got a real-looking answer here would
//! then have its own `InitProducerId(transactional_id=...)` refused by the
//! very node `FindCoordinator` just vouched for. Refusing here instead keeps
//! the two handlers' answers consistent with each other.

#![allow(clippy::redundant_pub_crate)]

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::find_coordinator::{FindCoordinatorResponse, decode_request, encode_response};
use oqueue_codec::frame::{RequestPrelude, encode_response_header};

/// The `key_type` the wire schema reserves for a consumer group — the only
/// case this broker answers for.
const GROUP: i8 = 0;

/// Decodes, answers with this node's own identity for a group lookup (or
/// refuses a transaction lookup), and encodes — or closes on a malformed
/// body.
pub(crate) fn handle(cluster: &Cluster, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };

    let response = if request.key_type == GROUP {
        FindCoordinatorResponse {
            error_code: error_codes::NONE,
            error_message: None,
            node_id: cluster.node_id,
            host: &cluster.host,
            port: cluster.port,
        }
    } else {
        FindCoordinatorResponse {
            error_code: error_codes::INVALID_REQUEST,
            error_message: Some("this broker coordinates consumer groups only"),
            node_id: -1,
            host: "",
            port: -1,
        }
    };

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::FindCoordinator,
        version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, version, &response);
    HandlerResponse::Reply(out)
}

#[cfg(test)]
mod tests;
