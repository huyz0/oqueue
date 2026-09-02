//! `FindCoordinator` (10), v0-6 — `M4.3`/`M4.4`, `ADR-0033`.
//!
//! ⚠️ **Every group resolves to this node, unconditionally** — `ADR-0033`'s
//! own decision, made real: v1 has one coordinator, this one, so there is
//! nothing to look up. `M4.4`'s batched form (v4+, KIP-699) answers every
//! requested key the same way, independently — one key's own resolution
//! never depends on another's, since there is nothing here that could make
//! it fail differently.
//!
//! ⚠️ **`key_type != 0` (`TRANSACTION`, from v1) is refused, not answered as
//! if understood** — `init_producer_id.rs`'s own precedent: `InitProducerId`
//! already refuses a `transactional_id` with `INVALID_REQUEST` because FR-15
//! (transactions) is deferred and nothing here backs the guarantee. Found by
//! `M4.3`'s own round 1 review: answering a `TRANSACTION`-type lookup with
//! this node's own identity would tell a client this node coordinates its
//! transaction, only for `InitProducerId` to refuse the very id that answer
//! implied would work. `key_type` applies to the whole batch (the wire
//! schema carries it once, not per key), so the refusal is all-or-nothing
//! across every key in one request.

#![allow(clippy::redundant_pub_crate)]

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::find_coordinator::{
    Coordinator, FindCoordinatorResponse, decode_request, encode_response,
};
use oqueue_codec::frame::{RequestPrelude, encode_response_header};

/// The `key_type` the wire schema reserves for a consumer group — the only
/// case this broker answers for.
const GROUP: i8 = 0;

/// This node's own answer for `key`, for a group lookup.
fn resolved<'a>(cluster: &'a Cluster, key: &'a str) -> Coordinator<'a> {
    Coordinator {
        key,
        error_code: error_codes::NONE,
        error_message: None,
        node_id: cluster.node_id,
        host: &cluster.host,
        port: cluster.port,
    }
}

/// The refusal every key in a `TRANSACTION`-type request gets, uniformly.
const fn refused(key: &str) -> Coordinator<'_> {
    Coordinator {
        key,
        error_code: error_codes::INVALID_REQUEST,
        error_message: Some("this broker coordinates consumer groups only"),
        node_id: -1,
        host: "",
        port: -1,
    }
}

/// Decodes, answers every requested key with this node's own identity for a
/// group lookup (or a refusal for a transaction lookup), and encodes — or
/// closes on a malformed body.
pub(crate) fn handle(cluster: &Cluster, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };

    let coordinators = request
        .keys
        .iter()
        .map(|&key| {
            if request.key_type == GROUP {
                resolved(cluster, key)
            } else {
                refused(key)
            }
        })
        .collect();
    let response = FindCoordinatorResponse { coordinators };

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
