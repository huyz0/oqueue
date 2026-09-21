//! The broker's deliberately supported API/version matrix.

use super::Advertised;
use crate::apikey::ApiKey;

/// Every API this broker advertises, in the order used by `ApiVersions`.
pub static ADVERTISED: [Advertised; 24] = [
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
        api_key: ApiKey::OffsetCommit,
        min: 2,
        max: 9,
        flexible_from: Some(8),
    },
    Advertised {
        api_key: ApiKey::OffsetFetch,
        min: 1,
        max: 7,
        flexible_from: Some(6),
    },
    Advertised {
        api_key: ApiKey::FindCoordinator,
        min: 0,
        max: 6,
        flexible_from: Some(3),
    },
    Advertised {
        api_key: ApiKey::JoinGroup,
        min: 0,
        max: 9,
        flexible_from: Some(6),
    },
    Advertised {
        api_key: ApiKey::Heartbeat,
        min: 0,
        max: 4,
        flexible_from: Some(4),
    },
    Advertised {
        api_key: ApiKey::LeaveGroup,
        min: 0,
        max: 5,
        flexible_from: Some(4),
    },
    Advertised {
        api_key: ApiKey::SyncGroup,
        min: 0,
        max: 5,
        flexible_from: Some(4),
    },
    Advertised {
        api_key: ApiKey::DescribeGroups,
        min: 0,
        max: 5,
        flexible_from: Some(5),
    },
    Advertised {
        api_key: ApiKey::ListGroups,
        min: 0,
        max: 4,
        flexible_from: Some(3),
    },
    Advertised {
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
        api_key: ApiKey::CreateTopics,
        min: 2,
        max: 7,
        flexible_from: Some(5),
    },
    Advertised {
        api_key: ApiKey::DeleteTopics,
        min: 1,
        max: 6,
        flexible_from: Some(4),
    },
    Advertised {
        api_key: ApiKey::DescribeConfigs,
        min: 1,
        max: 4,
        flexible_from: Some(4),
    },
    Advertised {
        api_key: ApiKey::AlterConfigs,
        min: 0,
        max: 2,
        flexible_from: Some(2),
    },
    Advertised {
        api_key: ApiKey::IncrementalAlterConfigs,
        min: 0,
        max: 1,
        flexible_from: Some(1),
    },
    Advertised {
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
    Advertised {
        api_key: ApiKey::DescribeClientQuotas,
        min: 0,
        max: 1,
        flexible_from: Some(1),
    },
    Advertised {
        api_key: ApiKey::AlterClientQuotas,
        min: 0,
        max: 1,
        flexible_from: Some(1),
    },
];
