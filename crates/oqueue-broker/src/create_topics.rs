//! The `CreateTopics` admin API (`M12.3`) over the durable catalog.

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{AdminAuthzContext, admin_authorized};
use crate::cluster::Cluster;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::create_topics::{
    CreateTopicsResponse, CreateTopicsResponseTopic, decode_request, encode_response,
};
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_core::{AdminOperation, TopicId};
use std::time::Duration;

const MAX_TOPIC_NAME_LEN: usize = 249;
const MAX_PARTITIONS: u32 = 1_000;

/// Decodes, authorizes, validates, and creates catalog entries.
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
    let authorized = admin_authorized(AdminOperation::CreateTopics, authz);
    let principal = authz.principal;
    let deadline = deadline_at(
        tokio::time::Instant::now(),
        timeout_duration(request.timeout_ms),
    );
    let mut topics = Vec::with_capacity(request.topics.len());
    for requested in request.topics {
        let now = tokio::time::Instant::now();
        let context = CreateContext {
            cluster,
            create_timeout: Some(remaining_until(deadline, now)),
            validate_only: request.validate_only,
            authorized,
            principal,
            topic_grants,
        };
        topics.push(create_one(&requested, &context).await);
    }
    let response = CreateTopicsResponse {
        throttle_time_ms: 0,
        topics,
    };
    let mut out = Vec::new();
    encode_response_header(
        &mut out,
        ApiKey::CreateTopics,
        prelude.api_version,
        prelude.correlation_id,
    )
    .ok()?;
    encode_response(&mut out, prelude.api_version, &response);
    Some(out)
}

struct CreateContext<'a> {
    cluster: &'a Cluster,
    create_timeout: Option<Duration>,
    validate_only: bool,
    authorized: bool,
    principal: Option<&'a oqueue_core::Principal>,
    topic_grants: &'a std::sync::RwLock<oqueue_core::TopicGrants>,
}

async fn create_one(
    requested: &oqueue_codec::create_topics::CreatableTopic,
    context: &CreateContext<'_>,
) -> CreateTopicsResponseTopic {
    let invalid = |code: i16, message: &str| CreateTopicsResponseTopic {
        name: requested.name.clone(),
        topic_id: [0; 16],
        error_code: code,
        error_message: Some(message.to_owned()),
        num_partitions: 0,
        replication_factor: 0,
    };
    if !context.authorized {
        return invalid(
            error_codes::TOPIC_AUTHORIZATION_FAILED,
            "CreateTopics is not authorized",
        );
    }
    let (name, partitions) = match validate(requested) {
        Ok(valid) => valid,
        Err((code, message)) => return invalid(code, message),
    };
    if context.validate_only {
        return success(&name, partitions, requested.replication_factor, [0; 16]);
    }
    let _topic_lifecycle = context.cluster.topic_lifecycle_read().await;
    let persisted = match persist(&name, partitions, context).await {
        Ok(Some(entry)) => entry,
        Ok(None) => return invalid(error_codes::UNKNOWN_SERVER_ERROR, "catalog write failed"),
        Err(()) => return invalid(error_codes::REQUEST_TIMED_OUT, "CreateTopics timed out"),
    };
    let (entry, created) = persisted;
    if !created {
        return already_exists(
            &name,
            entry.partitions(),
            requested.replication_factor,
            uuid_bytes(entry.id()),
        );
    }
    grant_created(context, &name);
    success(
        &name,
        entry.partitions(),
        requested.replication_factor,
        uuid_bytes(entry.id()),
    )
}

async fn persist(
    name: &TopicId,
    partitions: u32,
    context: &CreateContext<'_>,
) -> Result<Option<(oqueue_core::CatalogEntry, bool)>, ()> {
    let create = async {
        if let Some(principal) = context.principal {
            context
                .cluster
                .create_topic_owned_while_locked(name, partitions, principal)
                .await
                .map(|outcome| (outcome.entry().clone(), outcome.created()))
        } else {
            context
                .cluster
                .create_topic_while_locked(name, partitions)
                .await
                .map(|entry| (entry, true))
        }
    };
    let entry = match context.create_timeout {
        Some(timeout) => match tokio::time::timeout(timeout, create).await {
            Ok(entry) => entry,
            Err(_) => return Err(()),
        },
        None => create.await,
    };
    Ok(entry)
}

fn grant_created(context: &CreateContext<'_>, name: &TopicId) {
    if let (Some(principal), Ok(mut grants)) = (context.principal, context.topic_grants.write()) {
        grants.grant(principal.clone(), name.clone());
    }
}

fn already_exists(
    name: &TopicId,
    partitions: u32,
    replication_factor: i16,
    topic_id: [u8; 16],
) -> CreateTopicsResponseTopic {
    let mut response = success(name, partitions, replication_factor, topic_id);
    response.error_code = error_codes::TOPIC_ALREADY_EXISTS;
    response.error_message = Some("topic already exists".to_owned());
    response
}

fn timeout_duration(timeout_ms: i32) -> Duration {
    Duration::from_millis(u64::try_from(timeout_ms.max(0)).unwrap_or(0))
}

fn deadline_at(now: tokio::time::Instant, timeout: Duration) -> tokio::time::Instant {
    now + timeout
}

fn remaining_until(deadline: tokio::time::Instant, now: tokio::time::Instant) -> Duration {
    deadline.saturating_duration_since(now)
}

fn validate(
    requested: &oqueue_codec::create_topics::CreatableTopic,
) -> Result<(TopicId, u32), (i16, &'static str)> {
    if !valid_topic_name(&requested.name) {
        return Err((error_codes::INVALID_TOPIC_EXCEPTION, "invalid topic name"));
    }
    let Some(name) = TopicId::new(requested.name.clone()).ok() else {
        return Err((error_codes::INVALID_TOPIC_EXCEPTION, "invalid topic name"));
    };
    let partitions = if requested.num_partitions == -1 {
        1
    } else {
        let Ok(partitions) = u32::try_from(requested.num_partitions) else {
            return Err((error_codes::INVALID_PARTITIONS, "invalid partition count"));
        };
        if partitions == 0 || partitions > MAX_PARTITIONS {
            return Err((error_codes::INVALID_PARTITIONS, "invalid partition count"));
        }
        partitions
    };
    if requested.replication_factor != -1 && requested.replication_factor != 1 {
        return Err((
            error_codes::INVALID_REPLICATION_FACTOR,
            "single-node broker requires replication factor -1 or 1",
        ));
    }
    if !requested.assignments.is_empty() || !requested.configs.is_empty() {
        return Err((
            error_codes::INVALID_REQUEST,
            "manual assignments and topic configs are unsupported",
        ));
    }
    Ok((name, partitions))
}

fn valid_topic_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOPIC_NAME_LEN
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn success(
    name: &TopicId,
    partitions: u32,
    replication_factor: i16,
    topic_id: [u8; 16],
) -> CreateTopicsResponseTopic {
    CreateTopicsResponseTopic {
        name: name.as_str().to_owned(),
        topic_id,
        error_code: error_codes::NONE,
        error_message: None,
        num_partitions: i32::try_from(partitions).unwrap_or(i32::MAX),
        replication_factor: if replication_factor == -1 {
            1
        } else {
            replication_factor
        },
    }
}

const fn uuid_bytes(id: u128) -> [u8; 16] {
    id.to_be_bytes()
}

#[cfg(test)]
mod tests;
