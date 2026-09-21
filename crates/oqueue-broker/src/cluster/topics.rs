//! A cluster's view of the topic catalog (`ADR-0049` point 2): looked up
//! through [`TopicCatalog`] on a miss, cached only for topics this node has
//! served, and revalidated when the short cache lease expires because another
//! broker may have acknowledged a durable deletion.
//!
//! ⚠️ **The cache is not the catalog.** It holds what this node resolved,
//! never every topic that exists; a topic created elsewhere is found by
//! asking the catalog, not by waiting for it to appear here. The cache is a
//! latency optimization, not a deletion authority.

use super::Cluster;
use oqueue_core::{
    CatalogEntry, KeyDomain, Principal, TopicCreateOutcome, TopicDeleteOutcome, TopicId,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

/// How many names one `list` page asks for in [`Cluster::topic_names`].
const LIST_PAGE: usize = 1000;
const TOPIC_CACHE_TTL: Duration = Duration::from_secs(1);

/// One served topic: its id and how many partitions it has.
#[derive(Debug, Clone)]
struct Served {
    id: Uuid,
    partitions: usize,
    key_domain: KeyDomain,
    cached_at: Instant,
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

    /// The topic's key domain, or `None` if the topic cannot be resolved.
    ///
    /// ⚠️ This is metadata, not an inference from object bytes. A read must
    /// have this answer before it can decide whether a region is allowed to
    /// remain unsealed.
    pub(crate) async fn topic_key_domain(&self, topic: &TopicId) -> Option<KeyDomain> {
        self.served(topic.as_str()).await.map(|t| t.key_domain)
    }

    /// The name behind a topic id, or `None` — how the id-addressed APIs
    /// (`Produce` v13, `Fetch` v13+) resolve their targets. One catalog
    /// `lookup_id` on a miss, never a scan.
    pub async fn topic_name_by_id(&self, id: Uuid) -> Option<String> {
        if let Some(name) = self.with_cache(|c| c.by_id.get(&id).cloned()) {
            if self.with_cache(|c| c.by_name.get(&name).is_some_and(cache_is_fresh)) {
                return Some(name);
            }
            let topic = TopicId::new(&name).ok()?;
            self.forget_name(&topic, id.as_u128());
        }
        let generation = self.cache_generation.load(Ordering::Acquire);
        // ⚠️ A catalog error reads as "no such topic": see `served`.
        let entry = self.catalog.lookup_id(id.as_u128()).await.ok()??;
        self.remember_if_generation(&entry, generation)
    }

    /// Up to `limit` topic names, in name order — what an unscoped `Metadata`
    /// with no filter is answered with (`ADR-0049` point 4).
    ///
    /// ⚠️ **Bounded by `limit`, never by the catalog** (`M7.4`): it pages
    /// until it holds `limit` names or the catalog runs out, so its cost is
    /// O(`limit`) whatever the catalog holds. `topic_lookups` counts each name
    /// returned — `M9.17`'s proxy, which now stays flat past the bound.
    /// ⚠️ A catalog error ends the listing early, with what was paged so far.
    pub async fn topic_names(&self, limit: usize) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut after: Option<TopicId> = None;
        while names.len() < limit {
            let want = LIST_PAGE.min(limit - names.len());
            let Ok(page) = self.catalog.list(after.as_ref(), want).await else {
                break;
            };
            // An empty page ends the walk whatever was asked for, so no
            // request for nothing can spin.
            let full = !page.is_empty() && page.len() == want;
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
        let _lifecycle = self.topic_lifecycle_read().await;
        let Ok(name) = TopicId::new(topic) else {
            return false;
        };
        let generation = self.cache_generation.load(Ordering::Acquire);
        let Ok(entry) = self.catalog.create(&name, 1).await else {
            return false;
        };
        let _ = self.remember_if_generation(&entry, generation);
        true
    }

    /// Creates a catalog entry without starting a partition, log, or task.
    pub async fn create_topic(&self, topic: &TopicId, partitions: u32) -> Option<CatalogEntry> {
        let _lifecycle = self.topic_lifecycle_read().await;
        self.create_topic_while_locked(topic, partitions).await
    }

    /// Creates a topic while the caller holds the topic lifecycle read lock.
    pub(crate) async fn create_topic_while_locked(
        &self,
        topic: &TopicId,
        partitions: u32,
    ) -> Option<CatalogEntry> {
        let generation = self.cache_generation.load(Ordering::Acquire);
        let entry = self.catalog.create(topic, partitions).await.ok()?;
        let _ = self.remember_if_generation(&entry, generation);
        Some(entry)
    }

    /// Creates a topic on behalf of an authenticated principal, preserving
    /// whether this caller won the durable conditional write.
    pub async fn create_topic_owned(
        &self,
        topic: &TopicId,
        partitions: u32,
        creator: &Principal,
    ) -> Option<TopicCreateOutcome> {
        let _lifecycle = self.topic_lifecycle_read().await;
        self.create_topic_owned_while_locked(topic, partitions, creator)
            .await
    }

    /// Creates an owned topic while the caller holds the lifecycle read lock.
    pub(crate) async fn create_topic_owned_while_locked(
        &self,
        topic: &TopicId,
        partitions: u32,
        creator: &Principal,
    ) -> Option<TopicCreateOutcome> {
        let generation = self.cache_generation.load(Ordering::Acquire);
        let outcome = self
            .catalog
            .create_owned(topic, partitions, creator)
            .await
            .ok()?;
        let _ = self.remember_if_generation(outcome.entry(), generation);
        if outcome.created() {
            self.invalidate_creator_topics(creator);
        }
        Some(outcome)
    }

    /// Durably tombstones a topic and evicts every cache that can make it
    /// visible after deletion.
    ///
    /// # Errors
    ///
    /// Returns the catalog error when the durable tombstone or its index
    /// cleanup cannot be acknowledged.
    pub async fn delete_topic(
        &self,
        name: &TopicId,
        expected_id: Option<u128>,
    ) -> oqueue_core::Result<TopicDeleteOutcome> {
        let _lifecycle = self.topic_lifecycle.write().await;
        let outcome = match self.catalog.delete(name, expected_id).await {
            Ok(outcome) => outcome,
            Err(error) => {
                // The tombstone is published before index cleanup. If cleanup
                // failed, confirm that durable absence is already visible and
                // evict this node's serving state before returning the error.
                if self
                    .catalog
                    .lookup(name)
                    .await
                    .ok()
                    .is_some_and(|entry| entry.is_none())
                {
                    self.evict_durably_absent_topic(name);
                }
                return Err(error);
            }
        };
        match &outcome {
            TopicDeleteOutcome::Deleted(entry) => {
                self.forget(entry);
                if let Some(creator) = entry.creator() {
                    self.invalidate_creator_topics(creator);
                }
            }
            TopicDeleteOutcome::AlreadyDeleted { id } => {
                self.forget_name(name, *id);
                self.invalidate_all_creator_topics();
            }
            TopicDeleteOutcome::Missing | TopicDeleteOutcome::StaleId { .. } => {}
        }
        Ok(outcome)
    }

    pub(crate) async fn topic_lifecycle_read(&self) -> tokio::sync::OwnedRwLockReadGuard<()> {
        Arc::clone(&self.topic_lifecycle).read_owned().await
    }

    /// Confirms durable absence without consulting the serving cache.
    pub(crate) async fn topic_is_durably_absent(&self, name: &TopicId) -> Option<bool> {
        self.catalog
            .lookup(name)
            .await
            .ok()
            .map(|entry| entry.is_none())
    }

    pub(crate) fn evict_durably_absent_topic(&self, name: &TopicId) {
        self.forget_topic_name(name);
        self.invalidate_all_creator_topics();
    }

    /// Restores the durable creator grants for one principal. The caller
    /// performs the actual grant update so the broker's shared live index
    /// remains outside this cluster's catalog seam.
    pub async fn creator_topics(&self, creator: &Principal) -> Option<Vec<TopicId>> {
        let load = self
            .creator_topic_loads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(creator.clone())
            .or_insert_with(|| Arc::new(tokio::sync::OnceCell::new()))
            .clone();
        let result = load
            .get_or_init(|| async { self.load_creator_topics(creator).await })
            .await;
        let topics = result.as_ref().ok().cloned();
        if topics.is_none() {
            let mut loads = self
                .creator_topic_loads
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if loads
                .get(creator)
                .is_some_and(|current| Arc::ptr_eq(current, &load))
            {
                loads.remove(creator);
            }
        }
        topics
    }

    pub(crate) fn invalidate_creator_topics(&self, creator: &Principal) {
        self.creator_topic_loads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(creator);
    }

    fn invalidate_all_creator_topics(&self) {
        self.creator_topic_loads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    async fn load_creator_topics(&self, creator: &Principal) -> Result<Vec<TopicId>, ()> {
        let mut topics = Vec::new();
        let mut after = None;
        loop {
            let page = self
                .catalog
                .list_owned(creator, after.as_ref(), 1_000)
                .await
                .map_err(|_| ())?;
            let full = creator_page_is_full(page.len());
            after = page.last().cloned();
            topics.extend(page);
            if creator_page_is_done(full, after.as_ref()) {
                return Ok(topics);
            }
        }
    }

    /// How many topics this node's cache holds.
    #[cfg(test)]
    pub(crate) fn cached_topics(&self) -> usize {
        self.with_cache(|c| c.by_name.len())
    }

    #[cfg(test)]
    pub(crate) fn expire_topic_cache_for_test(&self) {
        self.with_cache(|cache| {
            let expired = Instant::now() - TOPIC_CACHE_TTL;
            for served in cache.by_name.values_mut() {
                served.cached_at = expired;
            }
        });
    }

    /// The cached entry for `topic`, or the catalog's on a miss.
    ///
    /// ⚠️ **A catalog error is answered as "not found"**, which the handlers
    /// turn into `UNKNOWN_TOPIC_OR_PARTITION` — retriable, so a client asks
    /// again rather than giving up, and nothing is cached on the way.
    async fn served(&self, topic: &str) -> Option<Served> {
        if let Some(hit) = self.with_cache(|c| c.by_name.get(topic).cloned()) {
            if cache_is_fresh(&hit) {
                return Some(hit);
            }
            let name = TopicId::new(topic).ok()?;
            self.forget_name(&name, hit.id.as_u128());
        }
        let name = TopicId::new(topic).ok()?;
        let generation = self.cache_generation.load(Ordering::Acquire);
        let entry = self.catalog.lookup(&name).await.ok()??;
        let _ = self.remember_if_generation(&entry, generation);
        self.with_cache(|c| c.by_name.get(topic).cloned())
    }

    /// Caches `entry`, returning its name.
    fn remember_if_generation(&self, entry: &CatalogEntry, generation: u64) -> Option<String> {
        let name = entry.name().as_str().to_owned();
        let served = Served {
            id: Uuid::from_u128(entry.id()),
            partitions: usize::try_from(entry.partitions()).unwrap_or(usize::MAX),
            key_domain: entry.key_domain().clone(),
            cached_at: Instant::now(),
        };
        self.with_cache(|c| {
            if self.cache_generation.load(Ordering::Acquire) != generation {
                return None;
            }
            c.by_id.insert(served.id, name.clone());
            c.by_name.insert(name.clone(), served);
            Some(name)
        })
    }

    fn forget(&self, entry: &CatalogEntry) {
        self.with_cache(|cache| {
            self.cache_generation.fetch_add(1, Ordering::AcqRel);
            let id = Uuid::from_u128(entry.id());
            cache.by_name.remove(entry.name().as_str());
            cache.by_id.remove(&id);
        });
    }

    fn forget_name(&self, name: &TopicId, id: u128) {
        self.with_cache(|cache| {
            self.cache_generation.fetch_add(1, Ordering::AcqRel);
            cache.by_name.remove(name.as_str());
            cache.by_id.remove(&Uuid::from_u128(id));
        });
    }

    fn forget_topic_name(&self, name: &TopicId) {
        self.with_cache(|cache| {
            self.cache_generation.fetch_add(1, Ordering::AcqRel);
            if let Some(served) = cache.by_name.remove(name.as_str()) {
                cache.by_id.remove(&served.id);
            }
        });
    }

    fn with_cache<T>(&self, f: impl FnOnce(&mut TopicCache) -> T) -> T {
        f(&mut self.topics.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn cache_is_fresh(served: &Served) -> bool {
    served.cached_at.elapsed() < TOPIC_CACHE_TTL
}

const fn creator_page_is_full(len: usize) -> bool {
    len == LIST_PAGE
}

const fn creator_page_is_done(full: bool, after: Option<&TopicId>) -> bool {
    !full || after.is_none()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{creator_page_is_done, creator_page_is_full};
    use oqueue_core::TopicId;

    #[test]
    fn creator_page_fullness_and_end_conditions_are_exact() {
        let topic = TopicId::new("last").expect("topic");
        assert!(creator_page_is_full(1_000));
        assert!(!creator_page_is_full(999));
        assert!(!creator_page_is_done(true, Some(&topic)));
        assert!(creator_page_is_done(false, Some(&topic)));
        assert!(creator_page_is_done(true, None));
    }
}
