//! The `IncrementalAlterConfigs` admin API (`M12.6`).

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{AdminAuthzContext, AuthzContext, admin_authorized, topic_authorized};
use crate::cluster::Cluster;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::incremental_alter_configs::{
    ConfigOperation, IncrementalAlterConfigsResponse, IncrementalConfigsResource, decode_request,
    encode_response,
};
use oqueue_compact::DELETION_DELAY_MS;
use oqueue_core::{AdminOperation, TopicId, TopicRetentionUpdate};

const TOPIC_RESOURCE: i8 = 2;
const RETENTION_MS: &str = "retention.ms";

/// Decodes, authorizes, validates, persists, and journals incremental changes.
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
    let authorized = admin_authorized(AdminOperation::AlterConfigs, admin_authz);
    let _lifecycle = cluster.topic_lifecycle_write().await;
    let mut resources = Vec::new();
    for resource in &request.resources {
        resources.push(
            alter_one(
                cluster,
                resource,
                authorized,
                topic_authz,
                request.validate_only,
            )
            .await,
        );
    }
    let response = IncrementalAlterConfigsResponse {
        throttle_time_ms: 0,
        resources,
    };
    let mut out = Vec::new();
    encode_response_header(
        &mut out,
        ApiKey::IncrementalAlterConfigs,
        prelude.api_version,
        prelude.correlation_id,
    )
    .ok()?;
    encode_response(&mut out, prelude.api_version, &response);
    Some(out)
}

#[allow(clippy::too_many_lines)]
async fn alter_one(
    cluster: &Cluster,
    resource: &IncrementalConfigsResource,
    authorized: bool,
    topic_authz: &AuthzContext<'_>,
    validate_only: bool,
) -> oqueue_codec::alter_configs::AlterConfigsResponseResource {
    let invalid = |error_code: i16, message: &str| {
        oqueue_codec::alter_configs::AlterConfigsResponseResource {
            error_code,
            error_message: Some(message.to_owned()),
            resource_type: resource.resource_type,
            resource_name: resource.resource_name.clone(),
        }
    };
    if resource.resource_type != TOPIC_RESOURCE {
        return invalid(error_codes::INVALID_REQUEST, "resource type is unsupported");
    }
    if !authorized || !topic_authorized(&resource.resource_name, topic_authz) {
        return invalid(
            error_codes::TOPIC_AUTHORIZATION_FAILED,
            "AlterConfigs is not authorized",
        );
    }
    if cluster
        .partition_count(&resource.resource_name)
        .await
        .is_none()
    {
        return invalid(error_codes::UNKNOWN_TOPIC_OR_PARTITION, "unknown topic");
    }
    let Ok(value) = incremental_value(resource) else {
        return invalid(error_codes::INVALID_CONFIG, "configuration is unsupported");
    };
    if validate_only {
        return success(resource);
    }
    let Some(topic) = TopicId::new(resource.resource_name.clone()).ok() else {
        return invalid(error_codes::INVALID_REQUEST, "invalid topic name");
    };
    let Ok(update) = persist(cluster, &topic, value).await else {
        return invalid(
            error_codes::UNKNOWN_SERVER_ERROR,
            "configuration was not persisted",
        );
    };
    if !matches!(update, TopicRetentionUpdate::Applied(_)) {
        return invalid(error_codes::UNKNOWN_TOPIC_OR_PARTITION, "unknown topic");
    }
    if journal(cluster, topic, value).await.is_err() {
        return invalid(
            error_codes::UNKNOWN_SERVER_ERROR,
            "configuration event was not journaled",
        );
    }
    success(resource)
}

async fn persist(
    cluster: &Cluster,
    topic: &TopicId,
    retention_ms: Option<i64>,
) -> Result<TopicRetentionUpdate, ()> {
    cluster
        .set_topic_retention_ms_while_locked(topic, retention_ms)
        .await
        .map_err(|_| ())
}

async fn journal(cluster: &Cluster, topic: TopicId, retention_ms: Option<i64>) -> Result<(), ()> {
    cluster
        .journal_topic_retention(topic, retention_ms)
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests;

fn incremental_value(resource: &IncrementalConfigsResource) -> Result<Option<i64>, ()> {
    if resource.configs.len() != 1 || resource.configs[0].name != RETENTION_MS {
        return Err(());
    }
    let config = &resource.configs[0];
    match config.operation {
        ConfigOperation::Set => {
            let value = config
                .value
                .as_deref()
                .ok_or(())?
                .parse::<i64>()
                .map_err(|_| ())?;
            (value >= DELETION_DELAY_MS)
                .then_some(Some(value))
                .ok_or(())
        }
        ConfigOperation::Delete => Ok(None),
        ConfigOperation::Append | ConfigOperation::Subtract => Err(()),
    }
}

fn success(
    resource: &IncrementalConfigsResource,
) -> oqueue_codec::alter_configs::AlterConfigsResponseResource {
    oqueue_codec::alter_configs::AlterConfigsResponseResource {
        error_code: error_codes::NONE,
        error_message: None,
        resource_type: resource.resource_type,
        resource_name: resource.resource_name.clone(),
    }
}
