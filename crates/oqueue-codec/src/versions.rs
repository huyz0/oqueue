//! What this broker advertises, per API — and the pins that hold the
//! dependency to it.
//!
//! What to advertise is *our* decision (`FR-2` binds it to what is
//! implemented); the flexible-versions cutovers are the protocol's.
//! ⚠️ **No global cutover exists** — Produce goes flexible at v9, Fetch at
//! v12, `ApiVersions` at v3 — so the table records each one and a
//! differential test below holds our [`crate::apikey::ApiKey`] header-version
//! logic to `kafka-protocol`'s generated answer (the `ADR-0019` oracle), at
//! every advertised version. A dependency bump that moves a cutover, or our
//! own logic drifting from it, fails here rather than against a live client.

use crate::apikey::ApiKey;

/// One advertised API: the version range this broker serves, and where the
/// wire goes flexible.
///
/// `None` — never — is `SaslHandshake`'s own row (`M9.3`): the protocol's
/// standing example for the case, now an instance of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advertised {
    /// The API this row advertises.
    pub api_key: ApiKey,
    /// Lowest version served.
    pub min: i16,
    /// Highest version served.
    pub max: i16,
    /// First version whose encoding is flexible (compact strings/arrays,
    /// tagged fields) — the per-API cutover the module doc warns about.
    pub flexible_from: Option<i16>,
}

/// Every API this broker advertises, in ascending `api_key` order.
///
/// ⚠️ Produce v0-v2 were removed by KIP-896 and are not advertised; Fetch
/// stops at v17 (the row's number) although the dependency can encode v18 —
/// advertising tracks what `M2.23`/`M2.24` implement and `M2.25`'s harness
/// exercises, never the dependency's ceiling.
pub static ADVERTISED: [Advertised; 15] = [
    Advertised {
        api_key: ApiKey::Produce,
        min: 3,
        max: 13,
        flexible_from: Some(9),
    },
    Advertised {
        api_key: ApiKey::Fetch,
        min: 4,
        max: 17,
        flexible_from: Some(12),
    },
    Advertised {
        // ⚠️ **From v1, not v0.** v0 answers an *array* of offsets per
        // partition — the pre-KIP-79 shape, where a client asked for N and got
        // a list — and nothing since Kafka 0.10 sends it. Advertising a
        // version means serving it (FR-2, `matrix.rs`), so the floor is where
        // the single-offset response begins.
        api_key: ApiKey::ListOffsets,
        min: 1,
        max: 9,
        flexible_from: Some(6),
    },
    Advertised {
        api_key: ApiKey::Metadata,
        min: 0,
        max: 13,
        flexible_from: Some(9),
    },
    Advertised {
        // ⚠️ **From v2, not v0** — the dependency's own generated
        // `OffsetCommitRequest` only models v2-9 at all (confirmed against
        // its generated source): v0/v1 carried a different shape
        // (`retention_time_ms` absent, a `timestamp` field per partition
        // instead) nothing since Kafka 0.11 sends. Ceiling v9 is the
        // *request's* own ceiling, not the response's (which the
        // dependency models to v10) — a version this broker cannot decode
        // a request for is not one it serves, however far the response
        // shape alone could reach.
        api_key: ApiKey::OffsetCommit,
        min: 2,
        max: 9,
        flexible_from: Some(8),
    },
    Advertised {
        // ⚠️ **From v1, not v0** — the dependency's own generated
        // `OffsetFetchRequest` only models v1-9 at all (confirmed against
        // its generated source): v0 answered a response shape with no
        // per-partition `error_code`, nothing since Kafka 0.10.2 sends.
        // Ceiling v7, not the dependency's v9 — v8-9 (KIP-709) batches
        // multiple *groups* per request, a materially different feature
        // from every other batched form this crate serves (which batch
        // items of the same kind within one group), `oqueue_codec::offset_fetch`'s
        // own doc.
        api_key: ApiKey::OffsetFetch,
        min: 1,
        max: 7,
        flexible_from: Some(6),
    },
    Advertised {
        // ⚠️ v0-3 single-key (`M4.3`), v4-6 batched (KIP-699, `M4.4`) —
        // one function, two frame shapes, `oqueue_codec::find_coordinator`'s
        // own doc. The dependency's own ceiling is v6 and every field from
        // v4 stays "Supported API versions: 4-6" uniformly (confirmed by
        // reading its generated source), so there is no InitProducerId-style
        // reason to advertise less than the full range.
        api_key: ApiKey::FindCoordinator,
        min: 0,
        max: 6,
        flexible_from: Some(3),
    },
    Advertised {
        // ⚠️ **v0-9, flexible from v6 — not this crate's usual v2/v3
        // cutover** — `oqueue_codec::join_group`'s own doc, confirmed
        // against the dependency's generated source directly (`M4.5`). The
        // codec's own ceiling is v9; nothing about this broker's handler
        // (`M4.7`, no `skip_assignment`/no server-side assignor) narrows it
        // the way `InitProducerId`'s v4 cap does.
        api_key: ApiKey::JoinGroup,
        min: 0,
        max: 9,
        flexible_from: Some(6),
    },
    Advertised {
        // ⚠️ **v0-4, flexible from v4** — `oqueue_codec::heartbeat`'s own
        // doc, confirmed against the dependency's generated source
        // directly (`M4.9`).
        api_key: ApiKey::Heartbeat,
        min: 0,
        max: 4,
        flexible_from: Some(4),
    },
    Advertised {
        // ⚠️ **v0-5, flexible from v4** — `oqueue_codec::leave_group`'s own
        // doc, confirmed against the dependency's generated source
        // directly (`M4.10`): batched from v3 (real Kafka's own batching,
        // ahead of `FindCoordinator`'s KIP-699 one), one function two
        // frame shapes, `find_coordinator.rs`'s own precedent.
        api_key: ApiKey::LeaveGroup,
        min: 0,
        max: 5,
        flexible_from: Some(4),
    },
    Advertised {
        // ⚠️ **v0-5, flexible from v4** — `oqueue_codec::sync_group`'s own
        // doc, confirmed against the dependency's generated source
        // directly (`M4.8`): a third cutover version in three consecutive
        // rows (v3 `FindCoordinator`, v6 `JoinGroup`, v4 here) is why this
        // crate reads the dependency rather than pattern-matching itself.
        api_key: ApiKey::SyncGroup,
        min: 0,
        max: 5,
        flexible_from: Some(4),
    },
    Advertised {
        // ⚠️ **Never flexible, at any version** — the dependency's own
        // `SaslHandshakeRequest::header_version` returns `1` regardless of
        // `version` (`apikey.rs`'s own doc, `M9.3`). `flexible_from: None`
        // is what makes that fall out of `ApiKey::is_flexible` rather than
        // needing a second special case beside `ApiVersions`'s own.
        api_key: ApiKey::SaslHandshake,
        min: 0,
        max: 1,
        flexible_from: None,
    },
    Advertised {
        api_key: ApiKey::ApiVersions,
        min: 0,
        max: 3,
        flexible_from: Some(3),
    },
    Advertised {
        // ⚠️ **Through v4, not the dependency's v5 ceiling.** v5 adds
        // `enable_2_pc`/`keep_prepared_txn`, both "Supported API versions:
        // none" in the schema (a future KIP's placeholder, not yet wire-
        // active at any version) — advertising it would promise nothing
        // this broker's decoder does not already serve at v4. `M11.4`
        // is non-transactional only (FR-15, deferred); `producer_id`/
        // `producer_epoch` (v3+) are decoded and ignored rather than
        // gating the floor, since a client presenting them for a fresh,
        // non-transactional init is answered the same as one that does not.
        api_key: ApiKey::InitProducerId,
        min: 0,
        max: 4,
        flexible_from: Some(2),
    },
    Advertised {
        api_key: ApiKey::SaslAuthenticate,
        min: 0,
        max: 2,
        flexible_from: Some(2),
    },
];

/// The advertised row for `api_key`, or `None` for an API this broker does
/// not serve — the caller's cue to answer `UNSUPPORTED_VERSION`.
#[must_use]
pub fn advertised_for(api_key: ApiKey) -> Option<&'static Advertised> {
    ADVERTISED.iter().find(|a| a.api_key == api_key)
}

/// Is `version` of `api_key` within what this broker advertises?
#[must_use]
pub fn supports(api_key: ApiKey, version: i16) -> bool {
    advertised_for(api_key).is_some_and(|a| version >= a.min && version <= a.max)
}

#[cfg(test)]
mod tests {
    // Same justification the sibling test modules give: every `expect` is on
    // a value this table constructed from literals it controls.
    #![allow(clippy::expect_used)]

    use super::{ADVERTISED, Advertised, advertised_for, supports};
    use crate::apikey::ApiKey;
    use kafka_protocol::messages::{
        ApiVersionsRequest, ApiVersionsResponse, FetchRequest, FetchResponse,
        FindCoordinatorRequest, FindCoordinatorResponse, HeartbeatRequest, HeartbeatResponse,
        InitProducerIdRequest, InitProducerIdResponse, JoinGroupRequest, JoinGroupResponse,
        LeaveGroupRequest, LeaveGroupResponse, ListOffsetsRequest, ListOffsetsResponse,
        MetadataRequest, MetadataResponse, OffsetCommitRequest, OffsetCommitResponse,
        OffsetFetchRequest, OffsetFetchResponse, ProduceRequest, ProduceResponse,
        SaslAuthenticateRequest, SaslAuthenticateResponse, SaslHandshakeRequest,
        SaslHandshakeResponse, SyncGroupRequest, SyncGroupResponse,
    };
    use kafka_protocol::protocol::{HeaderVersion, Message};

    /// The `ADR-0019` oracle for header versions: our
    /// [`ApiKey::request_header_version`] and
    /// [`ApiKey::response_header_version`] must equal `kafka-protocol`'s
    /// generated `header_version` for the paired request and response types,
    /// at every advertised version. The `ApiVersions` response-header special
    /// case is the one this most needs to hold — the dependency's generated
    /// `ApiVersionsResponse::header_version` is 0 at every version, and so
    /// must ours.
    /// The dependency's own generated `(request, response)` header versions
    /// for `api_key` at `version` — split out of the test below purely to
    /// keep that function under the fifty-line limit, `metadata_handle`'s
    /// own precedent in `oqueue-broker`.
    fn dependency_header_versions(api_key: ApiKey, version: i16) -> (i16, i16) {
        match api_key {
            ApiKey::Produce => (
                ProduceRequest::header_version(version),
                ProduceResponse::header_version(version),
            ),
            ApiKey::ListOffsets => (
                ListOffsetsRequest::header_version(version),
                ListOffsetsResponse::header_version(version),
            ),
            ApiKey::Fetch => (
                FetchRequest::header_version(version),
                FetchResponse::header_version(version),
            ),
            ApiKey::Metadata => (
                MetadataRequest::header_version(version),
                MetadataResponse::header_version(version),
            ),
            ApiKey::OffsetCommit => (
                OffsetCommitRequest::header_version(version),
                OffsetCommitResponse::header_version(version),
            ),
            ApiKey::OffsetFetch => (
                OffsetFetchRequest::header_version(version),
                OffsetFetchResponse::header_version(version),
            ),
            ApiKey::FindCoordinator => (
                FindCoordinatorRequest::header_version(version),
                FindCoordinatorResponse::header_version(version),
            ),
            ApiKey::JoinGroup | ApiKey::Heartbeat | ApiKey::LeaveGroup | ApiKey::SyncGroup => {
                group_protocol_header_versions(api_key, version)
            }
            ApiKey::ApiVersions => (
                ApiVersionsRequest::header_version(version),
                ApiVersionsResponse::header_version(version),
            ),
            ApiKey::InitProducerId => (
                InitProducerIdRequest::header_version(version),
                InitProducerIdResponse::header_version(version),
            ),
            ApiKey::SaslHandshake => (
                SaslHandshakeRequest::header_version(version),
                SaslHandshakeResponse::header_version(version),
            ),
            ApiKey::SaslAuthenticate => (
                SaslAuthenticateRequest::header_version(version),
                SaslAuthenticateResponse::header_version(version),
            ),
        }
    }

    /// The four classic group-protocol messages' own header versions --
    /// pulled out of `dependency_header_versions` purely to keep that
    /// function under the fifty-line limit, `metadata_handle`'s own
    /// precedent.
    fn group_protocol_header_versions(api_key: ApiKey, version: i16) -> (i16, i16) {
        match api_key {
            ApiKey::JoinGroup => (
                JoinGroupRequest::header_version(version),
                JoinGroupResponse::header_version(version),
            ),
            ApiKey::Heartbeat => (
                HeartbeatRequest::header_version(version),
                HeartbeatResponse::header_version(version),
            ),
            ApiKey::LeaveGroup => (
                LeaveGroupRequest::header_version(version),
                LeaveGroupResponse::header_version(version),
            ),
            ApiKey::SyncGroup => (
                SyncGroupRequest::header_version(version),
                SyncGroupResponse::header_version(version),
            ),
            other => unreachable!("group_protocol_header_versions called for {other:?}"),
        }
    }

    #[test]
    fn our_header_versions_match_the_dependency() {
        for row in &ADVERTISED {
            for version in row.min..=row.max {
                let (req, resp) = dependency_header_versions(row.api_key, version);
                assert_eq!(
                    row.api_key.request_header_version(version),
                    req,
                    "{:?} v{version} request header",
                    row.api_key
                );
                assert_eq!(
                    row.api_key.response_header_version(version),
                    resp,
                    "{:?} v{version} response header",
                    row.api_key
                );
            }
        }
    }

    /// The golden pin: at each advertised cutover the request header goes
    /// v2 (flexible), and one version below it is still v1. This is the
    /// dependency's generated code being held to the table — a
    /// `kafka-protocol` bump that moves a cutover fails here.
    #[test]
    fn every_flexible_cutover_matches_the_dependency() {
        fn pin<T: HeaderVersion>(row: &Advertised) {
            let from = row
                .flexible_from
                .expect("this row's cutover exists -- never-flexible rows use pin_never_flexible");
            assert_eq!(
                T::header_version(from),
                2,
                "{:?} must be flexible from v{from}",
                row.api_key
            );
            if from > row.min {
                assert_eq!(
                    T::header_version(from - 1),
                    1,
                    "{:?} must be non-flexible below v{from}",
                    row.api_key
                );
            }
        }
        // The `flexible_from: None` counterpart: every advertised version
        // stays at header version 1, never 2 -- `SaslHandshake`'s own
        // permanent case, pinned rather than left to `pin`'s `.expect()`.
        fn pin_never_flexible<T: HeaderVersion>(row: &Advertised) {
            for version in row.min..=row.max {
                assert_eq!(
                    T::header_version(version),
                    1,
                    "{:?} v{version} must never go flexible",
                    row.api_key
                );
            }
        }
        for row in &ADVERTISED {
            match row.api_key {
                ApiKey::Produce => pin::<ProduceRequest>(row),
                ApiKey::Fetch => pin::<FetchRequest>(row),
                ApiKey::ListOffsets => pin::<ListOffsetsRequest>(row),
                ApiKey::Metadata => pin::<MetadataRequest>(row),
                ApiKey::OffsetCommit => pin::<OffsetCommitRequest>(row),
                ApiKey::OffsetFetch => pin::<OffsetFetchRequest>(row),
                ApiKey::FindCoordinator => pin::<FindCoordinatorRequest>(row),
                ApiKey::JoinGroup => pin::<JoinGroupRequest>(row),
                ApiKey::Heartbeat => pin::<HeartbeatRequest>(row),
                ApiKey::LeaveGroup => pin::<LeaveGroupRequest>(row),
                ApiKey::SyncGroup => pin::<SyncGroupRequest>(row),
                ApiKey::SaslHandshake => pin_never_flexible::<SaslHandshakeRequest>(row),
                ApiKey::ApiVersions => pin::<ApiVersionsRequest>(row),
                ApiKey::InitProducerId => pin::<InitProducerIdRequest>(row),
                ApiKey::SaslAuthenticate => pin::<SaslAuthenticateRequest>(row),
            }
        }
    }

    /// Advertising outside what the dependency can encode would promise
    /// clients bytes nothing can produce.
    #[test]
    fn every_advertised_range_is_within_the_dependency() {
        fn within<T: Message>(row: &Advertised) {
            assert!(
                row.min >= T::VERSIONS.min && row.max <= T::VERSIONS.max,
                "{:?} advertises {}..={} outside the dependency's {}..={}",
                row.api_key,
                row.min,
                row.max,
                T::VERSIONS.min,
                T::VERSIONS.max,
            );
        }
        for row in &ADVERTISED {
            match row.api_key {
                ApiKey::Produce => within::<ProduceRequest>(row),
                ApiKey::Fetch => within::<FetchRequest>(row),
                ApiKey::ListOffsets => within::<ListOffsetsRequest>(row),
                ApiKey::Metadata => within::<MetadataRequest>(row),
                ApiKey::OffsetCommit => within::<OffsetCommitRequest>(row),
                ApiKey::OffsetFetch => within::<OffsetFetchRequest>(row),
                ApiKey::FindCoordinator => within::<FindCoordinatorRequest>(row),
                ApiKey::JoinGroup => within::<JoinGroupRequest>(row),
                ApiKey::Heartbeat => within::<HeartbeatRequest>(row),
                ApiKey::LeaveGroup => within::<LeaveGroupRequest>(row),
                ApiKey::SyncGroup => within::<SyncGroupRequest>(row),
                ApiKey::SaslHandshake => within::<SaslHandshakeRequest>(row),
                ApiKey::ApiVersions => within::<ApiVersionsRequest>(row),
                ApiKey::InitProducerId => within::<InitProducerIdRequest>(row),
                ApiKey::SaslAuthenticate => within::<SaslAuthenticateRequest>(row),
            }
        }
    }

    #[test]
    fn the_table_is_sorted_unique_and_sane() {
        for pair in ADVERTISED.windows(2) {
            assert!(
                (pair[0].api_key as i16) < (pair[1].api_key as i16),
                "ascending api_key order, no duplicates"
            );
        }
        for row in &ADVERTISED {
            assert!(
                row.min <= row.max,
                "{:?} has an inverted range",
                row.api_key
            );
            if let Some(from) = row.flexible_from {
                assert!(
                    from >= row.min && from <= row.max + 1,
                    "{:?}'s cutover must touch its advertised range",
                    row.api_key
                );
            }
        }
    }

    #[test]
    fn lookups_answer_the_unsupported_version_question() {
        assert!(supports(ApiKey::Produce, 3));
        assert!(supports(ApiKey::Produce, 13));
        assert!(!supports(ApiKey::Produce, 2), "KIP-896 removed v0-v2");
        assert!(!supports(ApiKey::Produce, 14));
        assert!(!supports(ApiKey::ApiVersions, 4), "v4 is not advertised");
        // ⚠️ An API this broker does not serve can no longer even be named:
        // our `ApiKey` has only the served variants (`ADR-0019`), so
        // every variant is in the table and the unserved case is
        // `ApiKey::from_i16` returning `None` (pinned in `apikey`), not a
        // table miss. So this asserts the table's own completeness instead.
        for row in &ADVERTISED {
            assert!(
                advertised_for(row.api_key).is_some(),
                "{:?} is a served key and must have a row",
                row.api_key
            );
        }
    }
}
