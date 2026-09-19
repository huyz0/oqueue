//! Which metadata shard a topic belongs to (`ADR-0049` point 1).
//!
//! ⚠️ **A lookup, never a hash burned into a key.** Every per-shard object
//! prefix is derived from the [`MetadataShardId`] a [`ShardMap`] answers, so
//! moving a topic to another shard rewrites one map entry and no key format.

use std::collections::HashMap;

use crate::TopicId;

/// A metadata shard: one metadata log, one group log, one catalog prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetadataShardId(u32);

impl MetadataShardId {
    /// The first shard, and the only one M7 runs.
    pub const ZERO: Self = Self(0);

    /// The shard numbered `id`.
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// The shard's number.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Where this shard's metadata log and its lease live.
    #[must_use]
    pub fn metadata_prefix(self) -> String {
        format!("meta/{}", self.0)
    }

    /// Where this shard's consumer-group log lives.
    #[must_use]
    pub fn groups_prefix(self) -> String {
        format!("groups/{}", self.0)
    }

    /// Where this shard's topic catalog lives.
    #[must_use]
    pub fn catalog_prefix(self) -> String {
        format!("catalog/{}", self.0)
    }
}

/// Answers topic → shard.
///
/// ⚠️ **It holds only the topics moved off the default**, so its size follows
/// how many topics were rebalanced, never the catalog's.
#[derive(Debug, Clone)]
pub struct ShardMap {
    default: MetadataShardId,
    moved: HashMap<TopicId, MetadataShardId>,
}

impl ShardMap {
    /// A map sending every topic to `default`.
    #[must_use]
    pub fn single(default: MetadataShardId) -> Self {
        Self {
            default,
            moved: HashMap::new(),
        }
    }

    /// The shard `topic` belongs to.
    #[must_use]
    pub fn shard_of(&self, topic: &TopicId) -> MetadataShardId {
        self.moved.get(topic).copied().unwrap_or(self.default)
    }

    /// Moves `topic` to `shard`.
    pub fn assign(&mut self, topic: TopicId, shard: MetadataShardId) {
        if shard == self.default {
            self.moved.remove(&topic);
        } else {
            self.moved.insert(topic, shard);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn topic(name: &str) -> TopicId {
        TopicId::new(name).expect("a topic")
    }

    #[test]
    fn every_topic_starts_on_the_default_shard() {
        let map = ShardMap::single(MetadataShardId::ZERO);
        assert_eq!(map.shard_of(&topic("a")), MetadataShardId::ZERO);
        assert_eq!(
            map.shard_of(&topic("a")).metadata_prefix(),
            "meta/0",
            "the prefix M6 wrote as a literal"
        );
    }

    #[test]
    fn moving_a_topic_changes_the_prefix_it_derives_and_no_other() {
        let mut map = ShardMap::single(MetadataShardId::ZERO);
        map.assign(topic("a"), MetadataShardId::new(7));
        let moved = map.shard_of(&topic("a"));
        assert_eq!(moved.get(), 7);
        assert_eq!(moved.metadata_prefix(), "meta/7");
        assert_eq!(moved.groups_prefix(), "groups/7");
        assert_eq!(moved.catalog_prefix(), "catalog/7");
        assert_eq!(map.shard_of(&topic("b")), MetadataShardId::ZERO, "b stays");

        map.assign(topic("a"), MetadataShardId::ZERO);
        assert_eq!(
            map.shard_of(&topic("a")),
            MetadataShardId::ZERO,
            "and moves back"
        );
        assert!(
            map.moved.is_empty(),
            "holding nothing for a topic on the default"
        );
    }
}
