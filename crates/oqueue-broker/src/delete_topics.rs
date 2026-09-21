//! The `DeleteTopics` admin API (`M12.4`) over durable catalog tombstones.

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{AdminAuthzContext, admin_authorized};
use crate::cluster::Cluster;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::delete_topics::{
    DeletableTopic, DeleteTopicsResponse, DeleteTopicsResponseTopic, decode_request,
    encode_response,
};
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_core::{AdminOperation, TopicDeleteOutcome, TopicId};
use std::time::Duration;

/// Decodes, authorizes, and durably tombstones each requested topic.
pub(crate) async fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &AdminAuthzContext<'_>,
    topic_grants: &std::sync::RwLock<oqueue_core::TopicGrants>,
) -> crate::connection::HandlerResponse {
    answer(cluster, prelude, body, authz, topic_grants)
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
    authz: &AdminAuthzContext<'_>,
    topic_grants: &std::sync::RwLock<oqueue_core::TopicGrants>,
) -> Option<Vec<u8>> {
    let request = decode_request(body, prelude.api_version).ok()?;
    let authorized = admin_authorized(AdminOperation::DeleteTopics, authz);
    let deadline = tokio::time::Instant::now()
        + Duration::from_millis(u64::try_from(request.timeout_ms.max(0)).unwrap_or(0));
    let mut responses = Vec::with_capacity(request.topics.len());
    for requested in request.topics {
        responses.push(
            delete_one(
                cluster,
                &requested,
                prelude.api_version,
                authorized,
                topic_grants,
                deadline,
            )
            .await,
        );
    }
    let response = DeleteTopicsResponse {
        throttle_time_ms: 0,
        responses,
    };
    let mut out = Vec::new();
    encode_response_header(
        &mut out,
        ApiKey::DeleteTopics,
        prelude.api_version,
        prelude.correlation_id,
    )
    .ok()?;
    encode_response(&mut out, prelude.api_version, &response);
    Some(out)
}

// One protocol item owns validation, UUID resolution, deadline handling, and
// the typed catalog outcome; keeping that flow together makes each wire error
// path auditable.
#[allow(
    clippy::single_match_else,
    clippy::too_many_arguments,
    clippy::too_many_lines
)]
async fn delete_one(
    cluster: &Cluster,
    requested: &DeletableTopic,
    version: i16,
    authorized: bool,
    topic_grants: &std::sync::RwLock<oqueue_core::TopicGrants>,
    deadline: tokio::time::Instant,
) -> DeleteTopicsResponseTopic {
    let requested_id = requested.topic_id;
    let invalid = |error_code: i16, message: &str, name: Option<String>, id: [u8; 16]| {
        DeleteTopicsResponseTopic {
            name,
            topic_id: id,
            error_code,
            error_message: (version >= 5).then(|| message.to_owned()),
        }
    };
    if !authorized {
        return invalid(
            error_codes::TOPIC_AUTHORIZATION_FAILED,
            "DeleteTopics is not authorized",
            requested.name.clone(),
            requested_id,
        );
    }
    let expected_id = (version >= 6)
        .then(|| u128::from_be_bytes(requested_id))
        .filter(|id| *id != 0);
    let name = match requested.name.as_deref() {
        Some(name) => match TopicId::new(name.to_owned()) {
            Ok(name) => name,
            Err(_) => {
                return invalid(
                    error_codes::INVALID_TOPIC_EXCEPTION,
                    "invalid topic name",
                    requested.name.clone(),
                    requested_id,
                );
            }
        },
        None => {
            let Some(id) = expected_id else {
                return invalid(
                    error_codes::INVALID_REQUEST,
                    "a UUID-only delete requires a non-nil topic id",
                    None,
                    requested_id,
                );
            };
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let name = match tokio::time::timeout(
                remaining,
                cluster.topic_name_by_id(uuid::Uuid::from_u128(id)),
            )
            .await
            {
                Ok(Some(name)) => name,
                Ok(None) => {
                    return invalid(
                        error_codes::UNKNOWN_TOPIC_ID,
                        "unknown topic id",
                        None,
                        requested_id,
                    );
                }
                Err(_) => {
                    return invalid(
                        error_codes::REQUEST_TIMED_OUT,
                        "DeleteTopics timed out",
                        None,
                        requested_id,
                    );
                }
            };
            match TopicId::new(name) {
                Ok(name) => name,
                Err(_) => {
                    return invalid(
                        error_codes::INVALID_TOPIC_EXCEPTION,
                        "invalid topic name",
                        None,
                        requested_id,
                    );
                }
            }
        }
    };
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let outcome =
        match tokio::time::timeout(remaining, cluster.delete_topic(&name, expected_id)).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => {
                reconcile_interrupted_delete(cluster, topic_grants, &name).await;
                return invalid(
                    error_codes::UNKNOWN_SERVER_ERROR,
                    "catalog delete failed",
                    Some(name.as_str().to_owned()),
                    requested_id,
                );
            }
            Err(_) => {
                reconcile_interrupted_delete(cluster, topic_grants, &name).await;
                return invalid(
                    error_codes::REQUEST_TIMED_OUT,
                    "DeleteTopics timed out",
                    Some(name.as_str().to_owned()),
                    requested_id,
                );
            }
        };
    let (code, id, message) = match outcome {
        TopicDeleteOutcome::Deleted(entry) => {
            revoke_topic(topic_grants, &name);
            (error_codes::NONE, entry.id(), "")
        }
        TopicDeleteOutcome::AlreadyDeleted { id } => {
            revoke_topic(topic_grants, &name);
            (
                error_codes::UNKNOWN_TOPIC_OR_PARTITION,
                id,
                "topic is deleted",
            )
        }
        TopicDeleteOutcome::Missing => (
            error_codes::UNKNOWN_TOPIC_OR_PARTITION,
            expected_id.unwrap_or(0),
            "unknown topic",
        ),
        TopicDeleteOutcome::StaleId { actual, .. } => {
            (error_codes::UNKNOWN_TOPIC_ID, actual, "topic id is stale")
        }
    };
    invalid(
        code,
        message,
        Some(name.as_str().to_owned()),
        id.to_be_bytes(),
    )
}

async fn reconcile_interrupted_delete(
    cluster: &Cluster,
    topic_grants: &std::sync::RwLock<oqueue_core::TopicGrants>,
    name: &TopicId,
) {
    let confirmed = tokio::time::timeout(
        Duration::from_millis(1),
        cluster.topic_is_durably_absent(name),
    )
    .await
    .ok()
    .flatten()
        == Some(true);
    if confirmed {
        cluster.evict_durably_absent_topic(name);
        revoke_topic(topic_grants, name);
    }
}

fn revoke_topic(topic_grants: &std::sync::RwLock<oqueue_core::TopicGrants>, name: &TopicId) {
    if let Ok(mut grants) = topic_grants.write() {
        grants.revoke_topic(name);
    }
}

#[cfg(test)]
mod tests;
