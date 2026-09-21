//! The `DescribeConfigs` admin API (`M12.5`).

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{AdminAuthzContext, AuthzContext, admin_authorized, topic_authorized};
use crate::cluster::Cluster;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::describe_configs::{
    ConfigEntry, ConfigResource, DescribeConfigsResponse, DescribeConfigsResponseResource,
    decode_request, encode_response,
};
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_compact::DEFAULT_RETENTION_MS;
use oqueue_core::AdminOperation;

const TOPIC_RESOURCE: i8 = 2;
const DEFAULT_CONFIG_SOURCE: i8 = 5;
const RETENTION_MS: &str = "retention.ms";

/// Decodes, authorizes, resolves, and describes supported topic settings.
pub(crate) async fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    admin_authz: &AdminAuthzContext<'_>,
    topic_authz: &AuthzContext<'_>,
) -> crate::connection::HandlerResponse {
    answer(cluster, prelude, body, admin_authz, topic_authz)
        .await
        .map_or(
            crate::connection::HandlerResponse::Close,
            crate::connection::HandlerResponse::Reply,
        )
}

async fn answer(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    admin_authz: &AdminAuthzContext<'_>,
    topic_authz: &AuthzContext<'_>,
) -> Option<Vec<u8>> {
    let request = decode_request(body, prelude.api_version).ok()?;
    let authorized = admin_authorized(AdminOperation::DescribeConfigs, admin_authz);
    // Grow incrementally: the resource count is client-controlled, and each
    // decoded resource is larger than its one-byte minimum wire unit.
    let mut resources = Vec::new();
    for resource in request.resources {
        resources.push(describe_one(cluster, &resource, authorized, topic_authz).await);
    }
    let response = DescribeConfigsResponse {
        throttle_time_ms: 0,
        resources,
    };
    let mut out = Vec::new();
    encode_response_header(
        &mut out,
        ApiKey::DescribeConfigs,
        prelude.api_version,
        prelude.correlation_id,
    )
    .ok()?;
    encode_response(&mut out, prelude.api_version, &response);
    Some(out)
}

async fn describe_one(
    cluster: &Cluster,
    resource: &ConfigResource,
    authorized: bool,
    topic_authz: &AuthzContext<'_>,
) -> DescribeConfigsResponseResource {
    let invalid = |error_code: i16, message: &str| DescribeConfigsResponseResource {
        error_code,
        error_message: Some(message.to_owned()),
        resource_type: resource.resource_type,
        resource_name: resource.resource_name.clone(),
        configs: Vec::new(),
    };
    if resource.resource_type != TOPIC_RESOURCE {
        return invalid(error_codes::INVALID_REQUEST, "resource type is unsupported");
    }
    if !authorized || !topic_authorized(&resource.resource_name, topic_authz) {
        return invalid(
            error_codes::TOPIC_AUTHORIZATION_FAILED,
            "DescribeConfigs is not authorized",
        );
    }
    if cluster
        .partition_count(&resource.resource_name)
        .await
        .is_none()
    {
        return invalid(error_codes::UNKNOWN_TOPIC_OR_PARTITION, "unknown topic");
    }
    let Ok(configs) = supported_configs(resource.config_names.as_deref()) else {
        return invalid(error_codes::INVALID_CONFIG, "configuration is unsupported");
    };
    DescribeConfigsResponseResource {
        error_code: error_codes::NONE,
        error_message: None,
        resource_type: resource.resource_type,
        resource_name: resource.resource_name.clone(),
        configs,
    }
}

fn supported_configs(names: Option<&[String]>) -> Result<Vec<ConfigEntry>, ()> {
    let requested = names.unwrap_or(&[]);
    if requested.is_empty() && names.is_some() {
        return Ok(Vec::new());
    }
    if requested.iter().any(|name| name != RETENTION_MS) {
        return Err(());
    }
    Ok(vec![ConfigEntry {
        name: RETENTION_MS.to_owned(),
        value: Some(DEFAULT_RETENTION_MS.to_string()),
        read_only: true,
        config_source: DEFAULT_CONFIG_SOURCE,
        is_sensitive: false,
    }])
}

#[cfg(test)]
mod tests;
