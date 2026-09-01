//! `InitProducerId` (22): mint a fresh producer identity, or refuse a
//! transactional one — `M11.4`.
//!
//! ⚠️ **No coordinator round trip.** `ADR-0031`/`M11.1` made a `ProducerId`
//! deliberately non-sequential — unique for the producer's lifetime, not
//! drawn from a cluster-wide counter a single shard's allocator could not
//! give anyway (a producer's writes can span shards). So minting one needs
//! nothing from [`Cluster`](crate::Cluster): no lock, no shard lookup, no
//! commit.
//!
//! ⚠️ **Transactional is refused, not answered as if understood.** FR-15 is
//! deferred; a call carrying `transactional_id` gets [`INVALID_REQUEST`]
//! rather than a minted identity a transactional producer would then use
//! under guarantees this broker does not provide.
//!
//! [`INVALID_REQUEST`]: oqueue_codec::error_codes::INVALID_REQUEST

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the
// `pub` clippy's `redundant_pub_crate` asks for. `pub(crate)` is the
// visibility that is actually true here, so the lint that disagrees is the
// one allowed — the same trade `region.rs`, `fetch/target.rs` and
// `test_executor.rs` already make.
#![allow(clippy::redundant_pub_crate)]

use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::init_producer_id::{InitProducerIdResponse, decode_request, encode_response};
use oqueue_core::{ProducerEpoch, ProducerId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Distinguishes two identities minted in the same nanosecond by this
/// process — [`WriterId::mint`](crate::WriterId)'s own reason for keeping
/// one (`writer_id.rs`), and the same shape reused here rather than a second
/// design.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// Decodes, mints or refuses, and encodes — or closes on a malformed body.
pub(crate) fn handle(prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };

    let response = if request.transactional_id.is_some() {
        InitProducerIdResponse {
            error_code: error_codes::INVALID_REQUEST,
            producer_id: -1,
            producer_epoch: -1,
        }
    } else {
        let id = mint_producer_id();
        InitProducerIdResponse {
            error_code: error_codes::NONE,
            producer_id: id.get(),
            producer_epoch: ProducerEpoch::ZERO.get(),
        }
    };

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::InitProducerId,
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

/// Mints a fresh, unforgeable producer identity.
///
/// ⚠️ **Process id, real clock, and an in-process counter** —
/// [`WriterId::mint`](crate::WriterId)'s own precedent (`writer_id.rs`),
/// which `producer_id.rs`'s own doc points here for exactly this reason:
/// `oqueue-core` is sans-I/O and cannot mint an identity itself. Colliding
/// *within one process* needs the same process to mint twice in the same
/// nanosecond, which `MINTED` still separates.
///
/// ⚠️ **This does not distinguish two different processes**, and is honest
/// about that rather than implying otherwise — review found the gap
/// (`M11.4`). Two brokers minting their first identity (`MINTED` at `0` in
/// both) in the same nanosecond can fold to the same value; unlike
/// `WriterId::mint`'s string concatenation, where a differing component
/// always yields a differing string, `fold`'s XOR-and-mask can equate two
/// different `(pid, stamp, nth)` triples. Not fixed here: the one thing
/// that would genuinely close it — a real per-broker identity mixed in —
/// does not exist yet. [`Cluster::node_id`](crate::Cluster) is hardcoded to
/// `0` for every process today (`cluster.rs`: "A single-node broker has a
/// single honest identity; the fleet answer belongs to the milestone that
/// has a fleet"), so threading it into `fold` now would mix in a constant
/// and change nothing. Not reachable in today's single-node deployment
/// shape for the same reason `Cluster::node_id` itself needs no fleet
/// answer yet; argued in `baselines/review.txt` rather than a fabricated
/// fix, and lands the moment a real per-broker id does.
///
/// ⚠️ **The real clock, deliberately, not the `Clock` seam** — same
/// reasoning `WriterId::mint` gives: a faked clock is shared by every
/// broker a test builds, which would make two identities collide exactly
/// where a test means to prove they do not. `check-sans-io.sh` exempts this
/// crate from the clock pattern for the same reason.
///
/// ⚠️ **Masked into the non-negative half of `i64`, not asserted into it.**
/// The mask (`& 0x7FFF_FFFF_FFFF_FFFF`) clears the sign bit unconditionally,
/// so [`ProducerId::new`] cannot actually see a negative input here — but
/// `code-structure.md` rule 26 bans `.unwrap()`/`.expect()` regardless of
/// how certain that is, so the fallback is a real value
/// ([`ProducerId::ZERO`]), never a panic.
fn mint_producer_id() -> ProducerId {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_u64, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    let nth = MINTED.fetch_add(1, Ordering::Relaxed);
    let pid = u64::from(std::process::id());
    ProducerId::new(fold(pid, stamp, nth)).unwrap_or(ProducerId::ZERO)
}

/// The entropy fold and sign-bit mask `mint_producer_id` applies to its
/// three raw ingredients — pulled into its own pure function so the exact
/// formula is pinned by [`fold_exercises_every_operator_the_formula_uses`]
/// rather than only observed through whether two calls happen to differ,
/// which an operator swap here (`^` for `|`, `&` for `^`) cannot fail —
/// `nth` alone already guarantees that (`testing.md` rule 17).
///
/// [`fold_exercises_every_operator_the_formula_uses`]: tests::fold_exercises_every_operator_the_formula_uses
const fn fold(pid: u64, stamp: u64, nth: u64) -> i64 {
    let folded = pid ^ stamp.rotate_left(19) ^ nth.rotate_left(41);
    (folded & 0x7FFF_FFFF_FFFF_FFFF).cast_signed()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::handle;
    use crate::connection::HandlerResponse;
    use kafka_protocol::messages::{
        InitProducerIdRequest, InitProducerIdResponse, TransactionalId,
    };
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
    use oqueue_codec::error_codes;
    use oqueue_codec::frame::RequestPrelude;

    const VERSION: i16 = 4;

    fn prelude() -> RequestPrelude {
        RequestPrelude {
            api_key: 22,
            api_version: VERSION,
            correlation_id: 7,
        }
    }

    fn replied(body: &[u8]) -> InitProducerIdResponse {
        let HandlerResponse::Reply(out) = handle(prelude(), body) else {
            panic!("a well-formed InitProducerId replies");
        };
        let mut rest = &out[5..]; // response header: correlation id + tagged fields
        let response = InitProducerIdResponse::decode(&mut rest, VERSION).expect("decodes");
        assert!(rest.is_empty());
        response
    }

    /// ⚠️ **A non-transactional call mints a real identity**: a fresh,
    /// non-negative `producer_id` at epoch zero — the idempotent-by-default
    /// case `M11.md`'s Goal names.
    #[test]
    fn a_non_transactional_call_mints_a_fresh_identity_at_epoch_zero() {
        let mut request = InitProducerIdRequest::default();
        request.transactional_id = None;
        request.transaction_timeout_ms = 30_000;
        let mut body = Vec::new();
        request.encode(&mut body, VERSION).expect("encodes");

        let response = replied(&body);
        assert_eq!(response.error_code, 0);
        assert!(response.producer_id.0 >= 0, "a minted id is never negative");
        assert_eq!(response.producer_epoch, 0);
    }

    /// ⚠️ **Two calls mint two distinct identities.** The same property
    /// `WriterId::mint`'s own test guards, one type over: a repeated id
    /// would let two unrelated producers share one sequence space.
    #[test]
    fn two_calls_mint_two_distinct_identities() {
        let mut request = InitProducerIdRequest::default();
        // ⚠️ `InitProducerIdRequest::default()`'s own `transactional_id` is
        // `Some("")`, not `None` — the dependency's generated `Default` for
        // a nullable string, not a null. `init_producer_id.rs`'s own
        // differential tests set this explicitly for the same reason.
        request.transactional_id = None;
        request.transaction_timeout_ms = 30_000;
        let mut body = Vec::new();
        request.encode(&mut body, VERSION).expect("encodes");

        let first = replied(&body).producer_id;
        let second = replied(&body).producer_id;
        assert_ne!(first, second);
    }

    /// ⚠️ **A transactional call is refused, not answered as if
    /// understood.** FR-15 is deferred; the sentinel identity comes back,
    /// never a minted one a transactional producer would then rely on under
    /// guarantees this broker does not provide.
    #[test]
    fn a_transactional_call_is_refused_with_the_sentinel_identity() {
        let mut request = InitProducerIdRequest::default();
        request.transactional_id = Some(TransactionalId(StrBytes::from_static_str("txn-1")));
        request.transaction_timeout_ms = 30_000;
        let mut body = Vec::new();
        request.encode(&mut body, VERSION).expect("encodes");

        let response = replied(&body);
        assert_eq!(response.error_code, error_codes::INVALID_REQUEST);
        assert_eq!(response.producer_id.0, -1);
        assert_eq!(response.producer_epoch, -1);
    }

    /// ⚠️ A malformed body closes, the dispatcher's policy for every
    /// unanswerable shape.
    #[test]
    fn a_malformed_body_closes_rather_than_panicking() {
        assert!(matches!(
            handle(prelude(), &[0xFF, 0xFF]),
            HandlerResponse::Close
        ));
    }

    /// ⚠️ **Pins `fold`'s exact formula, not just that two calls differ.**
    /// `pid = 3`, and `stamp`/`nth` are chosen so their rotated
    /// contributions are `5` and `6` — picked because `3 ^ 5 ^ 6 == 0`,
    /// the one value that makes every operator the real formula could have
    /// used instead diverge from it: `|` in place of either `^` gives a
    /// nonzero result (`3 | 5 == 7`, `5 | 6 == 7`), `&` in place of either
    /// gives a different nonzero one (`3 & 5 == 1`, `5 & 6 == 4`), and `^`
    /// in place of the final mask's `&` turns the all-zero fold into the
    /// mask itself (`i64::MAX`) instead of leaving it `0`. A "two calls
    /// differ" property test cannot fail under any of those swaps, because
    /// `nth` changing between calls already guarantees a different result
    /// regardless of which operator combined it in.
    #[test]
    fn fold_exercises_every_operator_the_formula_uses() {
        let stamp = 5u64.rotate_right(19);
        let nth = 6u64.rotate_right(41);
        assert_eq!(super::fold(3, stamp, nth), 0);
    }
}
