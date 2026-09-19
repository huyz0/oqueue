//! A cluster's view of the topic catalog (`ADR-0049` point 2): looked up
//! through [`TopicCatalog`] on a miss, cached only for topics this node has
//! served.
//!
//! ⚠️ **The cache is not the catalog.** It holds what this node resolved,
//! never every topic that exists; a topic created elsewhere is found by
//! asking the catalog, not by waiting for it to appear here.

use super::Cluster;
use oqueue_core::{CatalogEntry, TopicId};
use std::collections::HashMap;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use uuid::Uuid;

/// How many names one `list` page asks for in [`Cluster::topic_names`].
const LIST_PAGE: usize = 1000;

/// One served topic: its id and how many partitions it has.
#[derive(Debug, Clone, Copy)]
struct Served {
    id: Uuid,
    partitions: usize,
}

/// The topics this node has served, by name and by id.
#[derive(Debug, Default)]
pub(super) struct TopicCache {
    by_name: HashMap<String, Served>,
    by_id: HashMap<Uuid, String>,
}

impl Cluster {
    /// The topic's partition count, or `None` if it does not exist.
    pub async fn partition_count(&self, topic: &str) -> Option<usize> {
        self.topic_lookups.fetch_add(1, Ordering::Relaxed);
        self.served(topic).await.map(|t| t.partitions)
    }

    /// The topic's id, or `None` if it does not exist.
    pub async fn topic_id(&self, topic: &str) -> Option<Uuid> {
        self.served(topic).await.map(|t| t.id)
    }

    /// The name behind a topic id, or `None` — how the id-addressed APIs
    /// (`Produce` v13, `Fetch` v13+) resolve their targets. One catalog
    /// `lookup_id` on a miss, never a scan.
    pub async fn topic_name_by_id(&self, id: Uuid) -> Option<String> {
        if let Some(name) = self.with_cache(|c| c.by_id.get(&id).cloned()) {
            return Some(name);
        }
        // ⚠️ A catalog error reads as "no such topic": see `served`.
        let entry = self.catalog.lookup_id(id.as_u128()).await.ok()??;
        Some(self.remember(&entry))
    }

    /// Every topic name, sorted — `Metadata` with no filter asks for all.
    ///
    /// ⚠️ **O(catalog), not O(what a caller keeps)** — this pages through
    /// every entry, unlike [`Cluster::partition_count`]'s one targeted
    /// lookup. `topic_lookups` counts this proportionally to catalog size for
    /// exactly that reason: a caller that resolves a scoped set of names via
    /// `partition_count` per name costs O(that set); a caller that calls this
    /// and filters afterward is the O(catalog) anti-pattern `M9.10`'s own
    /// `all_topics_names` exists to avoid, and `M9.17`'s cost test needs a
    /// proxy that tells the two apart. ⚠️ A catalog error ends the listing
    /// early, with what was paged so far.
    pub async fn topic_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut after: Option<TopicId> = None;
        loop {
            let Ok(page) = self.catalog.list(after.as_ref(), LIST_PAGE).await else {
                break;
            };
            let full = page.len() == LIST_PAGE;
            names.extend(page.iter().map(|t| t.as_str().to_owned()));
            after = page.into_iter().last();
            if !full {
                break;
            }
        }
        self.topic_lookups.fetch_add(
            u64::try_from(names.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        // Already in name order by `list`'s contract; sorted anyway so the
        // guarantee is this method's rather than every catalog's.
        names.sort();
        names
    }

    /// Creates `topic` with one partition if absent. Returns whether it exists
    /// afterwards — false only for an empty name or a catalog failure.
    pub async fn ensure_topic(&self, topic: &str) -> bool {
        let Ok(name) = TopicId::new(topic) else {
            return false;
        };
        let Ok(entry) = self.catalog.create(&name, 1).await else {
            return false;
        };
        self.remember(&entry);
        true
    }

    /// How many topics this node's cache holds.
    #[cfg(test)]
    pub(crate) fn cached_topics(&self) -> usize {
        self.with_cache(|c| c.by_name.len())
    }

    /// The cached entry for `topic`, or the catalog's on a miss.
    ///
    /// ⚠️ **A catalog error is answered as "not found"**, which the handlers
    /// turn into `UNKNOWN_TOPIC_OR_PARTITION` — retriable, so a client asks
    /// again rather than giving up, and nothing is cached on the way.
    async fn served(&self, topic: &str) -> Option<Served> {
        if let Some(hit) = self.with_cache(|c| c.by_name.get(topic).copied()) {
            return Some(hit);
        }
        let name = TopicId::new(topic).ok()?;
        let entry = self.catalog.lookup(&name).await.ok()??;
        self.remember(&entry);
        self.with_cache(|c| c.by_name.get(topic).copied())
    }

    /// Caches `entry`, returning its name.
    fn remember(&self, entry: &CatalogEntry) -> String {
        let name = entry.name().as_str().to_owned();
        let served = Served {
            id: Uuid::from_u128(entry.id()),
            partitions: usize::try_from(entry.partitions()).unwrap_or(usize::MAX),
        };
        self.with_cache(|c| {
            c.by_id.insert(served.id, name.clone());
            c.by_name.insert(name.clone(), served);
        });
        name
    }

    fn with_cache<T>(&self, f: impl FnOnce(&mut TopicCache) -> T) -> T {
        f(&mut self.topics.lock().unwrap_or_else(PoisonError::into_inner))
    }
}
