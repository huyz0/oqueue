//! Principal quota administration (`M12.9`).

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{AdminAuthzContext, admin_authorized};
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::quota_admin::{
    AlterClientQuotasResponse, AlterClientQuotasResult, DescribeClientQuotasEntry,
    DescribeClientQuotasResponse, IN_FLIGHT_REQUESTS, QuotaEntity, QuotaFilter, USER_ENTITY,
    decode_alter_request, decode_describe_request, encode_alter_response, encode_describe_response,
};
use oqueue_core::{AdminOperation, Principal, PrincipalQuota};
use std::sync::Arc;

const EXACT: i8 = 0;
const ANY: i8 = 2;

/// Answers `DescribeClientQuotas` for the supported user/in-flight surface.
pub(crate) fn describe(
    quota: Option<&Arc<PrincipalQuota>>,
    prelude: RequestPrelude,
    body: &[u8],
    admin_authz: &AdminAuthzContext<'_>,
) -> HandlerResponse {
    let Ok(request) = decode_describe_request(body, prelude.api_version) else {
        return HandlerResponse::Close;
    };
    let authorized = admin_authorized(AdminOperation::AlterQuotas, admin_authz);
    let (error_code, error_message, entries) = if !authorized {
        (
            error_codes::CLUSTER_AUTHORIZATION_FAILED,
            Some("DescribeClientQuotas is not authorized".to_owned()),
            Vec::new(),
        )
    } else if let Some(quota) = quota {
        match describe_entries(quota, &request.components) {
            Ok(entries) => (error_codes::NONE, None, entries),
            Err(message) => (
                error_codes::INVALID_REQUEST,
                Some(message.to_owned()),
                Vec::new(),
            ),
        }
    } else {
        // No overrides are a valid description even when the optional live
        // admission policy was not configured at startup.
        (error_codes::NONE, None, Vec::new())
    };
    let response = DescribeClientQuotasResponse {
        throttle_time_ms: 0,
        error_code,
        error_message,
        entries,
    };
    let mut out = Vec::new();
    encode_response_header(
        &mut out,
        ApiKey::DescribeClientQuotas,
        prelude.api_version,
        prelude.correlation_id,
    )
    .ok();
    encode_describe_response(&mut out, prelude.api_version, &response);
    HandlerResponse::Reply(out)
}

/// Answers `AlterClientQuotas`, applying each valid entry to the shared live
/// quota policy unless the request is validation-only.
pub(crate) fn alter(
    quota: Option<&Arc<PrincipalQuota>>,
    prelude: RequestPrelude,
    body: &[u8],
    admin_authz: &AdminAuthzContext<'_>,
) -> HandlerResponse {
    let Ok(request) = decode_alter_request(body, prelude.api_version) else {
        return HandlerResponse::Close;
    };
    let authorized = admin_authorized(AdminOperation::AlterQuotas, admin_authz);
    let entries = request
        .entries
        .iter()
        .map(|entry| alter_one(quota, entry, authorized, request.validate_only))
        .collect();
    let response = AlterClientQuotasResponse {
        throttle_time_ms: 0,
        entries,
    };
    let mut out = Vec::new();
    encode_response_header(
        &mut out,
        ApiKey::AlterClientQuotas,
        prelude.api_version,
        prelude.correlation_id,
    )
    .ok();
    encode_alter_response(&mut out, prelude.api_version, &response);
    HandlerResponse::Reply(out)
}

fn describe_entries(
    quota: &PrincipalQuota,
    filters: &[QuotaFilter],
) -> Result<Vec<DescribeClientQuotasEntry>, &'static str> {
    for filter in filters {
        validate_filter(filter)?;
    }
    Ok(quota
        .overrides()
        .into_iter()
        .filter(|(principal, _)| {
            filters
                .iter()
                .all(|filter| filter_matches(filter, principal))
        })
        .map(|(principal, limit)| DescribeClientQuotasEntry {
            entity: vec![QuotaEntity {
                entity_type: USER_ENTITY.to_owned(),
                name: Some(principal.as_str().to_owned()),
            }],
            values: vec![oqueue_codec::quota_admin::QuotaValue {
                key: IN_FLIGHT_REQUESTS.to_owned(),
                value: f64::from(limit),
            }],
        })
        .collect())
}

fn validate_filter(filter: &QuotaFilter) -> Result<(), &'static str> {
    if filter.entity_type != USER_ENTITY {
        return Err("only user quota entities are supported");
    }
    match filter.match_type {
        EXACT if filter.name.as_deref().is_some_and(|name| !name.is_empty()) => Ok(()),
        ANY if filter.name.is_none() => Ok(()),
        _ => Err("quota filter must be an exact named user or any user"),
    }
}

fn filter_matches(filter: &QuotaFilter, principal: &Principal) -> bool {
    match filter.match_type {
        EXACT => filter.name.as_deref() == Some(principal.as_str()),
        ANY => true,
        _ => false,
    }
}

fn alter_one(
    quota: Option<&Arc<PrincipalQuota>>,
    entry: &oqueue_codec::quota_admin::AlterClientQuotasEntry,
    authorized: bool,
    validate_only: bool,
) -> AlterClientQuotasResult {
    let entity = entry.entity.clone();
    let failure = |error_code: i16, message: &'static str| AlterClientQuotasResult {
        error_code,
        error_message: Some(message.to_owned()),
        entity: entity.clone(),
    };
    if !authorized {
        return failure(
            error_codes::CLUSTER_AUTHORIZATION_FAILED,
            "AlterClientQuotas is not authorized",
        );
    }
    let Some(quota) = quota else {
        return failure(
            error_codes::INVALID_REQUEST,
            "client quotas are not configured",
        );
    };
    let Ok(principal) = entity_principal(&entry.entity) else {
        return failure(
            error_codes::INVALID_REQUEST,
            "quota entity must be one named user",
        );
    };
    let Some(operation) = entry.operations.as_slice().first() else {
        return failure(error_codes::INVALID_CONFIG, "quota operation is missing");
    };
    if entry.operations.len() != 1 || operation.key != IN_FLIGHT_REQUESTS {
        return failure(error_codes::INVALID_CONFIG, "quota key is unsupported");
    }
    let limit = if operation.remove {
        None
    } else {
        match quota_limit(operation.value) {
            Some(limit) => Some(limit),
            None => return failure(error_codes::INVALID_CONFIG, "quota value must be a u32"),
        }
    };
    if !validate_only {
        quota.set_override(principal, limit);
    }
    AlterClientQuotasResult {
        error_code: error_codes::NONE,
        error_message: None,
        entity,
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quota_limit(value: f64) -> Option<u32> {
    (value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX))
        .then_some(value as u32)
}

fn entity_principal(entities: &[QuotaEntity]) -> Result<Principal, ()> {
    if entities.len() != 1 || entities[0].entity_type != USER_ENTITY {
        return Err(());
    }
    entities[0]
        .name
        .as_deref()
        .ok_or(())
        .and_then(|name| Principal::new(name).map_err(|_| ()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{alter, describe, describe_entries, entity_principal, filter_matches, quota_limit};
    use crate::authz::AdminAuthzContext;
    use crate::connection::HandlerResponse;
    use kafka_protocol::messages::{AlterClientQuotasResponse, DescribeClientQuotasResponse};
    use kafka_protocol::protocol::Decodable;
    use oqueue_codec::flex::put_string;
    use oqueue_codec::frame::RequestPrelude;
    use oqueue_codec::quota_admin::{
        AlterClientQuotasEntry, QuotaEntity, QuotaFilter, QuotaOperation,
    };
    use oqueue_codec::wire::{put_bool, put_f64, put_i32};
    use oqueue_core::{AdminGrants, AdminOperation, Principal, PrincipalQuota};
    use std::sync::Arc;

    fn principal(name: &str) -> Principal {
        Principal::new(name).expect("valid principal")
    }

    fn alter_body(name: &str, value: f64, remove: bool) -> Vec<u8> {
        let mut body = Vec::new();
        put_i32(&mut body, 1);
        put_i32(&mut body, 1);
        put_string(&mut body, false, "user");
        put_string(&mut body, false, name);
        put_i32(&mut body, 1);
        put_string(&mut body, false, "in_flight_requests");
        put_f64(&mut body, value);
        put_bool(&mut body, remove);
        put_bool(&mut body, false);
        body
    }

    fn prelude(api_key: i16) -> RequestPrelude {
        RequestPrelude {
            api_key,
            api_version: 0,
            correlation_id: 9,
        }
    }

    #[test]
    fn an_authorized_update_changes_live_admission_and_keeps_bob_independent() {
        let quota = Arc::new(PrincipalQuota::new(4));
        let alice = principal("alice");
        let bob = principal("bob");
        let mut grants = AdminGrants::new();
        grants.grant(alice.clone(), AdminOperation::AlterQuotas);
        let authz = AdminAuthzContext {
            principal: Some(&alice),
            credentials_configured: true,
            admin_grants: &grants,
        };

        let reply = alter(
            Some(&quota),
            prelude(49),
            &alter_body("bob", 1.0, false),
            &authz,
        );
        let HandlerResponse::Reply(reply) = reply else {
            panic!("expected quota reply");
        };
        let mut body = &reply[4..];
        let response = AlterClientQuotasResponse::decode(&mut body, 0).expect("response");
        assert!(body.is_empty());
        assert_eq!(response.entries[0].error_code, 0);
        let _bob = PrincipalQuota::admit(&quota, &bob).expect("bob's first request");
        assert!(PrincipalQuota::admit(&quota, &bob).is_none());
        assert!(PrincipalQuota::admit(&quota, &alice).is_some());
    }

    #[test]
    fn unauthorized_alter_does_not_change_the_policy() {
        let quota = Arc::new(PrincipalQuota::new(4));
        let alice = principal("alice");
        let grants = AdminGrants::new();
        let authz = AdminAuthzContext {
            principal: Some(&alice),
            credentials_configured: true,
            admin_grants: &grants,
        };
        let reply = alter(
            Some(&quota),
            prelude(49),
            &alter_body("alice", 0.0, false),
            &authz,
        );
        let HandlerResponse::Reply(reply) = reply else {
            panic!("expected quota reply");
        };
        let mut body = &reply[4..];
        let response = AlterClientQuotasResponse::decode(&mut body, 0).expect("response");
        assert_eq!(response.entries[0].error_code, 31);
        assert!(quota.overrides().is_empty());
    }

    #[test]
    fn describe_returns_only_explicit_overrides() {
        let quota = Arc::new(PrincipalQuota::new(4));
        let alice = principal("alice");
        quota.set_override(alice.clone(), Some(2));
        let mut grants = AdminGrants::new();
        grants.grant(alice.clone(), AdminOperation::AlterQuotas);
        let authz = AdminAuthzContext {
            principal: Some(&alice),
            credentials_configured: true,
            admin_grants: &grants,
        };
        let mut body = Vec::new();
        put_i32(&mut body, 1);
        put_string(&mut body, false, "user");
        body.push(0);
        put_string(&mut body, false, "alice");
        put_bool(&mut body, false);
        let reply = describe(Some(&quota), prelude(48), &body, &authz);
        let HandlerResponse::Reply(reply) = reply else {
            panic!("expected quota reply");
        };
        let mut response_body = &reply[4..];
        let response =
            DescribeClientQuotasResponse::decode(&mut response_body, 0).expect("response");
        assert!(response_body.is_empty());
        assert_eq!(response.error_code, 0);
        assert_eq!(response.entries.expect("entries").len(), 1);
    }

    fn invalid_filters() -> [QuotaFilter; 5] {
        [
            QuotaFilter {
                entity_type: "client-id".to_owned(),
                match_type: super::EXACT,
                name: Some("alice".to_owned()),
            },
            QuotaFilter {
                entity_type: "user".to_owned(),
                match_type: super::EXACT,
                name: None,
            },
            QuotaFilter {
                entity_type: "user".to_owned(),
                match_type: super::EXACT,
                name: Some(String::new()),
            },
            QuotaFilter {
                entity_type: "user".to_owned(),
                match_type: super::ANY,
                name: Some("alice".to_owned()),
            },
            QuotaFilter {
                entity_type: "user".to_owned(),
                match_type: 1,
                name: None,
            },
        ]
    }

    #[test]
    fn quota_filters_validate_and_scope_descriptions() {
        let quota = PrincipalQuota::new(4);
        let alice = principal("alice");
        let bob = principal("bob");
        quota.set_override(alice.clone(), Some(2));
        quota.set_override(bob.clone(), Some(3));

        let exact = QuotaFilter {
            entity_type: "user".to_owned(),
            match_type: super::EXACT,
            name: Some("alice".to_owned()),
        };
        let any = QuotaFilter {
            entity_type: "user".to_owned(),
            match_type: super::ANY,
            name: None,
        };
        assert_eq!(
            describe_entries(&quota, std::slice::from_ref(&exact)).expect("exact"),
            vec![oqueue_codec::quota_admin::DescribeClientQuotasEntry {
                entity: vec![QuotaEntity {
                    entity_type: "user".to_owned(),
                    name: Some("alice".to_owned()),
                }],
                values: vec![oqueue_codec::quota_admin::QuotaValue {
                    key: "in_flight_requests".to_owned(),
                    value: 2.0,
                }],
            },]
        );
        assert_eq!(
            describe_entries(&quota, std::slice::from_ref(&any))
                .expect("any")
                .len(),
            2
        );
        assert!(filter_matches(&exact, &alice));
        assert!(!filter_matches(&exact, &bob));
        assert!(filter_matches(&any, &alice));

        for invalid in invalid_filters() {
            assert!(describe_entries(&quota, std::slice::from_ref(&invalid)).is_err());
        }
    }

    #[test]
    fn quota_values_and_entities_reject_invalid_shapes() {
        assert_eq!(quota_limit(f64::NAN), None);
        assert_eq!(quota_limit(f64::INFINITY), None);
        assert_eq!(quota_limit(-1.0), None);
        assert_eq!(quota_limit(1.5), None);
        assert_eq!(quota_limit(f64::from(u32::MAX) + 1.0), None);
        assert_eq!(quota_limit(0.0), Some(0));
        assert_eq!(quota_limit(f64::from(u32::MAX)), Some(u32::MAX));

        let user = |name: &str| QuotaEntity {
            entity_type: "user".to_owned(),
            name: Some(name.to_owned()),
        };
        assert_eq!(
            entity_principal(&[user("alice")]).expect("principal"),
            principal("alice")
        );
        assert!(
            entity_principal(&[QuotaEntity {
                entity_type: "client-id".to_owned(),
                name: Some("alice".to_owned()),
            }])
            .is_err()
        );
        assert!(entity_principal(&[user("alice"), user("bob")]).is_err());
        assert!(
            entity_principal(&[QuotaEntity {
                entity_type: "user".to_owned(),
                name: None,
            }])
            .is_err()
        );
        assert!(entity_principal(&[user("")]).is_err());
    }

    #[test]
    fn alter_rejects_multiple_or_unsupported_operations() {
        let quota = Arc::new(PrincipalQuota::new(4));
        let entity = vec![QuotaEntity {
            entity_type: "user".to_owned(),
            name: Some("alice".to_owned()),
        }];
        let operation = QuotaOperation {
            key: "in_flight_requests".to_owned(),
            value: 2.0,
            remove: false,
        };
        let multiple = AlterClientQuotasEntry {
            entity: entity.clone(),
            operations: vec![operation.clone(), operation.clone()],
        };
        let unsupported = AlterClientQuotasEntry {
            entity,
            operations: vec![QuotaOperation {
                key: "request_percentage".to_owned(),
                ..operation
            }],
        };
        assert_eq!(
            super::alter_one(Some(&quota), &multiple, true, false).error_code,
            oqueue_codec::error_codes::INVALID_CONFIG
        );
        assert_eq!(
            super::alter_one(Some(&quota), &unsupported, true, false).error_code,
            oqueue_codec::error_codes::INVALID_CONFIG
        );
        assert!(quota.overrides().is_empty());
    }
}
