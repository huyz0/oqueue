//! What this broker advertises, per API — and the pins that hold the
//! dependency to it.
//!
//! The flexible-versions rules are `kafka-protocol`'s (`ADR-0017`); ours is
//! the *decision* of what to advertise, which `FR-2` binds to what is
//! actually implemented. ⚠️ **No global flexible cutover exists** — Produce
//! goes flexible at v9, Fetch at v12, `ApiVersions` at v3 — and nothing
//! here computes those rules: the table records each cutover and the golden
//! tests below hold the dependency's generated code to them, so a
//! `kafka-protocol` bump that moves one fails here rather than against a
//! live client.

use kafka_protocol::messages::ApiKey;

/// One advertised API: the version range this broker serves, and where the
/// wire goes flexible (`None` — never — has no instance in this table yet,
/// but `SaslHandshake` is the protocol's standing example).
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
pub static ADVERTISED: [Advertised; 4] = [
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
        api_key: ApiKey::Metadata,
        min: 0,
        max: 13,
        flexible_from: Some(9),
    },
    Advertised {
        api_key: ApiKey::ApiVersions,
        min: 0,
        max: 3,
        flexible_from: Some(3),
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
    use kafka_protocol::messages::{
        ApiKey, ApiVersionsRequest, FetchRequest, MetadataRequest, ProduceRequest,
    };
    use kafka_protocol::protocol::{HeaderVersion, Message};

    /// The golden pin: at each advertised cutover the request header goes
    /// v2 (flexible), and one version below it is still v1. This is the
    /// dependency's generated code being held to the table — a
    /// `kafka-protocol` bump that moves a cutover fails here.
    #[test]
    fn every_flexible_cutover_matches_the_dependency() {
        fn pin<T: HeaderVersion>(row: &Advertised) {
            let from = row
                .flexible_from
                .expect("every advertised API in this table has a cutover");
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
        for row in &ADVERTISED {
            match row.api_key {
                ApiKey::Produce => pin::<ProduceRequest>(row),
                ApiKey::Fetch => pin::<FetchRequest>(row),
                ApiKey::Metadata => pin::<MetadataRequest>(row),
                ApiKey::ApiVersions => pin::<ApiVersionsRequest>(row),
                other => panic!("no pin written for advertised API {other:?}"),
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
                ApiKey::Metadata => within::<MetadataRequest>(row),
                ApiKey::ApiVersions => within::<ApiVersionsRequest>(row),
                other => panic!("no bound check written for advertised API {other:?}"),
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
        assert!(
            advertised_for(ApiKey::SaslHandshake).is_none(),
            "an API not in the table is the UNSUPPORTED_VERSION cue"
        );
    }
}
