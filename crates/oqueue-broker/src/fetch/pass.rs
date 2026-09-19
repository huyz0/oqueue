//! One pass over every partition a `Fetch` names.
//!
//! ⚠️ **Its own module because framing a response and producing one are
//! different jobs**, and only the second has a cost. `mod.rs` decodes the
//! request and encodes the reply; everything here decides what each partition
//! answers with and what asking cost the broker.
//!
//! ⚠️ **The two bounds have different scopes, and that is the module's one
//! subtlety.** The byte budget is per *pass*, because bytes bound the response
//! and a response is one pass's outcomes. The object cache is per *request*,
//! because GETs bound object storage and a parked fetch makes up to
//! `MAX_READS_PER_REQUEST` passes over the same offsets. [`read_all`] carries
//! the argument; `park.rs` is where the cache is created.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use super::TopicOutcome;
use super::partition::{self, one_partition};
use super::target::Budget;
use crate::authz::{AuthzContext, topic_authorized};
use crate::cluster::Cluster;
use crate::read::{FetchedObjects, Spend};
use oqueue_codec::error_codes;
use oqueue_core::{PartitionId, TopicId};

/// Every requested partition, read once.
///
/// ⚠️ **The budget is per *pass* and the object cache is per *request*, and
/// that asymmetry is deliberate.** They bound different things. Bytes bound the
/// **response**, and a response is one pass's outcomes — carrying a spent
/// budget into the next pass would answer a client empty because an earlier
/// pass, whose bytes it will never see, had already read that much. GETs bound
/// **object storage**, which the whole request pays for however many passes it
/// makes; a per-pass cache would let a parked fetch re-download the same bundle
/// once per wakeup, and `MAX_FAILED_FETCHES_PER_REQUEST` would mean two
/// failures per pass rather than two per request.
///
/// ⚠️ **The cache is also what keeps the per-pass budget from starving a
/// re-read.** A second pass over the same offsets is all cache hits, so it
/// fetches nothing and only genuinely new objects are charged again.
pub(crate) async fn read_all(
    cluster: &Cluster,
    request: &oqueue_codec::fetch::FetchRequest<'_>,
    version: i16,
    objects: &mut FetchedObjects,
    authz: &AuthzContext<'_>,
) -> Vec<TopicOutcome> {
    // ⚠️ **One budget for the whole pass**, threaded through every topic and
    // every partition in turn — a fresh one per topic would be the
    // per-partition bug with a different multiplier.
    let mut budget = Budget::new(request.max_bytes);
    let scope = TopicScope { version, authz };
    let mut outcomes: Vec<TopicOutcome> = Vec::with_capacity(request.topics.len());
    for topic in &request.topics {
        outcomes.push(one_topic(cluster, topic, &scope, &mut budget, objects).await);
    }
    outcomes
}

/// Where each requested partition's log currently ends.
///
/// ⚠️ **The cheap half of a re-read.** Every entry here is an index lookup, so
/// asking after every wakeup costs nothing an idle shard would notice — which
/// is what lets the expensive half happen only when something moved.
pub(crate) async fn watermarks(
    cluster: &Cluster,
    request: &oqueue_codec::fetch::FetchRequest<'_>,
    version: i16,
) -> Vec<i64> {
    let mut ends = Vec::new();
    for topic in &request.topics {
        let name = resolved_name(cluster, topic, version).await;
        for partition in &topic.partitions {
            // ⚠️ **Unresolvable entries are skipped, not given a sentinel.** A
            // topic this broker does not host is a refusal, and a refusal is
            // answered rather than parked on — so nothing here can be waiting
            // on one, and a sentinel would only be a value nothing compares.
            // Skipping is stable across a park because topics are never
            // removed, so the two vectors line up.
            if let Some(pair) = name
                .as_deref()
                .and_then(|name| TopicId::new(name.to_owned()).ok())
                .zip(PartitionId::new(partition.index).ok())
            {
                ends.push(cluster.high_watermark(&pair.0, pair.1).get());
            }
        }
    }
    ends
}

/// The topic this entry names, whichever way this version addresses topics.
///
/// From v13 the wire carries an id and this resolves it against the registry;
/// below that it carries the name itself.
pub(crate) async fn resolved_name(
    cluster: &Cluster,
    topic: &oqueue_codec::fetch::FetchTopic<'_>,
    version: i16,
) -> Option<String> {
    if version >= 13 {
        cluster
            .topic_name_by_id(uuid::Uuid::from_bytes(topic.topic_id))
            .await
    } else {
        topic.name.map(str::to_owned)
    }
}

/// The per-request scalars `one_topic` needs beyond `topic` itself, bundled
/// to stay under `rust-style.md`'s argument-count limit once `M9.12` added
/// `authz` beside them.
struct TopicScope<'a> {
    version: i16,
    authz: &'a AuthzContext<'a>,
}

/// One topic's outcome: resolve its addressing, then every partition.
async fn one_topic(
    cluster: &Cluster,
    topic: &oqueue_codec::fetch::FetchTopic<'_>,
    scope: &TopicScope<'_>,
    budget: &mut Budget,
    objects: &mut FetchedObjects,
) -> TopicOutcome {
    let version = scope.version;
    // From v13 the wire addresses topics by id — same split as produce:
    // echo what this version carries, resolve the rest.
    // ⚠️ At v13 the wire carries no name to echo, so the response repeats the
    // id and the name is resolved only to find the partition.
    let name = if version >= 13 {
        None
    } else {
        topic.name.map(str::to_owned)
    };
    let resolved_name = resolved_name(cluster, topic, version).await;
    // An id this broker never issued has its own error (100); a name it
    // does not host stays UNKNOWN_TOPIC_OR_PARTITION, matching what real
    // brokers answer on each addressing path.
    let unknown = if version >= 13 {
        error_codes::UNKNOWN_TOPIC_ID
    } else {
        error_codes::UNKNOWN_TOPIC_OR_PARTITION
    };
    // ⚠️ **`M9.12`: checked once per topic, before any partition runs** —
    // `None` (unresolvable, already `UNKNOWN_TOPIC_*`'s own case) counts as
    // authorized here so existence-refusal keeps its own unmodified meaning;
    // only a topic that genuinely exists and is genuinely ungranted is
    // refused by this check *at v13+*, where `resolved_name` came from
    // `Cluster::topic_name_by_id` and so is `None` exactly when the id does
    // not exist.
    //
    // ⚠️ **Below v13, `resolved_name` is the client's raw name, unresolved
    // — the same asymmetry `produce/mod.rs::one_topic`'s own doc names.**
    // `TopicGrants` is keyed by name, so a name-addressed request can be
    // authorized without existence ever being resolved first — `Metadata`'s
    // own explicit-name case (`M9.9`) does exactly this at every version it
    // serves. An id cannot be checked the same way: there is no name to
    // check `can_see` against until the id resolves to one, so
    // existence-first is not a policy choice at v13+, it is what makes a
    // name to check exist at all. The two paths differ by addressing mode,
    // not by an oversight in one of them.
    let authorized = resolved_name
        .as_deref()
        .is_none_or(|name| topic_authorized(name, scope.authz));
    let mut partitions = Vec::with_capacity(topic.partitions.len());
    for p in &topic.partitions {
        partitions.push(if authorized {
            // ⚠️ **The share this partition may spend, taken from the
            // request's own remaining budget.** A partition naming a
            // larger `partition_max_bytes` than the request has left gets
            // what is left: the request's number bounds the response a
            // client actually asked for.
            let allowance = budget.allowance(p.partition_max_bytes);
            let outcome = one_partition(
                cluster,
                resolved_name.as_deref(),
                unknown,
                p,
                &mut Spend { allowance, objects },
            )
            .await;
            budget.spend(outcome.cost, !outcome.records.is_empty());
            outcome
        } else {
            partition::refused(
                cluster,
                resolved_name.as_deref(),
                p.index,
                error_codes::TOPIC_AUTHORIZATION_FAILED,
            )
        });
    }
    TopicOutcome {
        name,
        topic_id: topic.topic_id,
        partitions,
    }
}

/// `M9.12`'s own tests: per-principal scoping on `Fetch`, `Metadata`'s
/// `M9.9` shape reused rather than reinvented.
#[cfg(test)]
mod authorization {
    #![allow(clippy::expect_used)]

    use crate::authz::AuthzContext;
    use crate::connection::HandlerResponse;
    use crate::fetch::handle;
    use crate::fetch::tests::{by_id, by_name, decode, fetch_body, prelude};
    use crate::testing::{fixture, golden_batch, produce_one};
    use oqueue_core::{Principal, TopicGrants, TopicId};

    fn alice() -> Principal {
        Principal::new("alice").expect("valid")
    }

    #[tokio::test]
    async fn an_unconfigured_broker_answers_every_topic_regardless_of_grants() {
        let fx = fixture(&["t"]).await;
        produce_one(&fx, "t", golden_batch()).await;
        let authz = AuthzContext {
            principal: None,
            credentials_configured: false,
            topic_grants: &TopicGrants::default(),
        };
        let HandlerResponse::Reply(out) = handle(
            &fx.cluster,
            &fx.session,
            prelude(13),
            &fetch_body(13, by_id(&fx).await, 0, 0),
            &authz,
        )
        .await
        else {
            panic!("expected a reply");
        };
        assert_eq!(decode(&out, 13).responses[0].partitions[0].error_code, 0);
    }

    #[tokio::test]
    async fn a_configured_broker_refuses_an_ungranted_topic() {
        let fx = fixture(&["t"]).await;
        produce_one(&fx, "t", golden_batch()).await;
        let grants = TopicGrants::new();
        let authz = AuthzContext {
            principal: Some(&alice()),
            credentials_configured: true,
            topic_grants: &grants,
        };
        let HandlerResponse::Reply(out) = handle(
            &fx.cluster,
            &fx.session,
            prelude(13),
            &fetch_body(13, by_id(&fx).await, 0, 0),
            &authz,
        )
        .await
        else {
            panic!("expected a reply");
        };
        assert_eq!(
            decode(&out, 13).responses[0].partitions[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
    }

    #[tokio::test]
    async fn a_configured_broker_answers_a_granted_topic() {
        let fx = fixture(&["t"]).await;
        produce_one(&fx, "t", golden_batch()).await;
        let mut grants = TopicGrants::new();
        grants.grant(alice(), TopicId::new("t").expect("valid"));
        let authz = AuthzContext {
            principal: Some(&alice()),
            credentials_configured: true,
            topic_grants: &grants,
        };
        let HandlerResponse::Reply(out) = handle(
            &fx.cluster,
            &fx.session,
            prelude(13),
            &fetch_body(13, by_id(&fx).await, 0, 0),
            &authz,
        )
        .await
        else {
            panic!("expected a reply");
        };
        let response = decode(&out, 13);
        assert_eq!(response.responses[0].partitions[0].error_code, 0);
        assert!(
            response.responses[0].partitions[0]
                .records
                .as_ref()
                .is_some_and(|r| !r.is_empty()),
            "a granted topic's fetch still reads real records"
        );
    }

    /// A refusal never touches object storage — the same "decided before a
    /// read costs nothing" property `partition.rs`'s own doc names for every
    /// other pre-read refusal.
    #[tokio::test]
    async fn refusing_an_ungranted_topic_touches_no_object_storage() {
        let fx = fixture(&["t"]).await;
        produce_one(&fx, "t", golden_batch()).await;
        let grants = TopicGrants::new();
        let authz = AuthzContext {
            principal: Some(&alice()),
            credentials_configured: true,
            topic_grants: &grants,
        };
        let _ = handle(
            &fx.cluster,
            &fx.session,
            prelude(13),
            &fetch_body(13, by_id(&fx).await, 0, 0),
            &authz,
        )
        .await;
        assert_eq!(
            fx.store.counts().count(oqueue_core::Operation::Get),
            0,
            "a refusal decided before a read costs nothing"
        );
    }

    /// ⚠️ **Round 1 review's own finding, pinned rather than left implicit.**
    /// Below v13 `resolved_name` is the client's raw, unresolved name, so a
    /// topic that was never created and is also ungranted answers
    /// `TOPIC_AUTHORIZATION_FAILED`, not `UNKNOWN_TOPIC_OR_PARTITION` — the
    /// opposite of what the same logical request gets at v13+ (`the
    /// `resolved_name`-is-`None` test below pins that side). Both are real,
    /// version-dependent, and now both are asserted rather than one of them
    /// being an accident nothing checks.
    #[tokio::test]
    async fn below_v13_a_nonexistent_ungranted_topic_answers_authorization_failed_not_unknown() {
        let fx = fixture(&[]).await;
        let grants = TopicGrants::new();
        let authz = AuthzContext {
            principal: Some(&alice()),
            credentials_configured: true,
            topic_grants: &grants,
        };
        let HandlerResponse::Reply(out) = handle(
            &fx.cluster,
            &fx.session,
            prelude(11),
            &fetch_body(11, by_name("ghost"), 0, 0),
            &authz,
        )
        .await
        else {
            panic!("expected a reply");
        };
        assert_eq!(
            decode(&out, 11).responses[0].partitions[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
            "below v13, authorization runs before existence is ever resolved"
        );
    }

    /// The same version, name-addressed, actually granted: still answers.
    #[tokio::test]
    async fn below_v13_a_granted_topic_answers() {
        let fx = fixture(&["t"]).await;
        produce_one(&fx, "t", golden_batch()).await;
        let mut grants = TopicGrants::new();
        grants.grant(alice(), TopicId::new("t").expect("valid"));
        let authz = AuthzContext {
            principal: Some(&alice()),
            credentials_configured: true,
            topic_grants: &grants,
        };
        let HandlerResponse::Reply(out) = handle(
            &fx.cluster,
            &fx.session,
            prelude(11),
            &fetch_body(11, by_name("t"), 0, 0),
            &authz,
        )
        .await
        else {
            panic!("expected a reply");
        };
        assert_eq!(decode(&out, 11).responses[0].partitions[0].error_code, 0);
    }

    /// The v13+ (id-addressed) side of the same asymmetry: an id that
    /// resolves to nothing answers `UNKNOWN_TOPIC_ID`, never
    /// `TOPIC_AUTHORIZATION_FAILED` — existence is resolved first here
    /// because there is no name to check authorization against until it
    /// is.
    #[tokio::test]
    async fn at_or_above_v13_an_unresolvable_id_answers_unknown_not_authorization_failed() {
        let fx = fixture(&[]).await;
        let grants = TopicGrants::new();
        let authz = AuthzContext {
            principal: Some(&alice()),
            credentials_configured: true,
            topic_grants: &grants,
        };
        let ghost = uuid::Uuid::from_u128(0xDEAD_BEEF);
        let HandlerResponse::Reply(out) = handle(
            &fx.cluster,
            &fx.session,
            prelude(13),
            &crate::fetch::tests::hungry_fetch_body(
                13,
                crate::fetch::tests::by_id_of(ghost),
                0,
                0,
                crate::fetch::tests::Poll {
                    max_wait_ms: 0,
                    min_bytes: 1,
                },
            ),
            &authz,
        )
        .await
        else {
            panic!("expected a reply");
        };
        assert_eq!(
            decode(&out, 13).responses[0].partitions[0].error_code,
            kafka_protocol::error::ResponseError::UnknownTopicId.code(),
            "existence is resolved before authorization at v13+"
        );
    }
}
