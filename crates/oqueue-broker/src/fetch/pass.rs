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
use super::partition::one_partition;
use super::target::Budget;
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
) -> Vec<TopicOutcome> {
    // ⚠️ **One budget for the whole pass**, threaded through every topic and
    // every partition in turn — a fresh one per topic would be the
    // per-partition bug with a different multiplier.
    let mut budget = Budget::new(request.max_bytes);
    let mut outcomes: Vec<TopicOutcome> = Vec::with_capacity(request.topics.len());
    for topic in &request.topics {
        outcomes.push(one_topic(cluster, topic, version, &mut budget, objects).await);
    }
    outcomes
}

/// Where each requested partition's log currently ends.
///
/// ⚠️ **The cheap half of a re-read.** Every entry here is an index lookup, so
/// asking after every wakeup costs nothing an idle shard would notice — which
/// is what lets the expensive half happen only when something moved.
pub(crate) fn watermarks(
    cluster: &Cluster,
    request: &oqueue_codec::fetch::FetchRequest<'_>,
    version: i16,
) -> Vec<i64> {
    let mut ends = Vec::new();
    for topic in &request.topics {
        let name = resolved_name(cluster, topic, version);
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
pub(crate) fn resolved_name(
    cluster: &Cluster,
    topic: &oqueue_codec::fetch::FetchTopic<'_>,
    version: i16,
) -> Option<String> {
    if version >= 13 {
        cluster.topic_name_by_id(uuid::Uuid::from_bytes(topic.topic_id))
    } else {
        topic.name.map(str::to_owned)
    }
}

/// One topic's outcome: resolve its addressing, then every partition.
async fn one_topic(
    cluster: &Cluster,
    topic: &oqueue_codec::fetch::FetchTopic<'_>,
    version: i16,
    budget: &mut Budget,
    objects: &mut FetchedObjects,
) -> TopicOutcome {
    // From v13 the wire addresses topics by id — same split as produce:
    // echo what this version carries, resolve the rest.
    // ⚠️ At v13 the wire carries no name to echo, so the response repeats the
    // id and the name is resolved only to find the partition.
    let name = if version >= 13 {
        None
    } else {
        topic.name.map(str::to_owned)
    };
    let resolved_name = resolved_name(cluster, topic, version);
    // An id this broker never issued has its own error (100); a name it
    // does not host stays UNKNOWN_TOPIC_OR_PARTITION, matching what real
    // brokers answer on each addressing path.
    let unknown = if version >= 13 {
        error_codes::UNKNOWN_TOPIC_ID
    } else {
        error_codes::UNKNOWN_TOPIC_OR_PARTITION
    };
    let mut partitions = Vec::with_capacity(topic.partitions.len());
    for p in &topic.partitions {
        partitions.push({
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
        });
    }
    TopicOutcome {
        name,
        topic_id: topic.topic_id,
        partitions,
    }
}
