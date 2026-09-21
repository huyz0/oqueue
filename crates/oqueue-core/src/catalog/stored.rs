//! The topic catalog in object storage (`ADR-0049` point 3, `M7.3`).
//!
//! Three live objects per owned topic, plus one permanent tombstone after
//! deletion, under the shard's catalog prefix:
//!
//! - `<prefix>/topic/<hex of the name's UTF-8 bytes>` — the entry;
//! - `<prefix>/id/<32 hex digits of the id>` — the name, so an id finds it.
//! - `<prefix>/owner/<hex creator>/<hex name>` — the durable owner index.
//! - `<prefix>/tombstone/<hex name>` — the reserved UUID after deletion.
//!
//! ⚠️ **Hex, not the name itself**: any [`TopicId`] makes a valid key, `/`
//! included, and lowercase hex of the bytes sorts exactly as the bytes do, so
//! a key listing *is* name order.

use std::sync::Arc;

use super::{CatalogEntry, TopicCatalog, TopicDeleteOutcome};
use crate::{
    BoxFuture, ByteRange, Error, KeyDomain, MaintenanceStore, MetadataShardId, ObjectKey,
    ObjectStore, Precondition, Principal, Result, TopicCreateOutcome, TopicId,
    TopicRetentionUpdate,
};

/// The entry format's version, the body's first byte.
const DEFAULT_FORMAT: u8 = 1;
const CUSTOMER_FORMAT: u8 = 2;
const RACE_VISIBILITY_ATTEMPTS: u32 = 64;
const OWNER_DEFAULT_FORMAT: u8 = 3;
const OWNER_CUSTOMER_FORMAT: u8 = 4;

mod config;
mod delete;
mod format;
use format::{decode, encode, hex, unhex};

/// A [`TopicCatalog`] over object storage, one shard's worth.
///
/// ⚠️ **Creation provisions nothing** (doc 15 §2): `create` writes exactly the
/// two live objects above and starts no task, timer or log. A topic costs its
/// catalog entry until something writes to it; deletion's tombstone is the
/// only durable object added by the admin lifecycle.
///
/// ⚠️ **The topic entry is written first**, so a crash before the id index is
/// complete leaves a recoverable entry rather than an orphan reservation. A
/// retry repairs the id index before the topic becomes visible.
#[derive(Debug)]
pub struct ObjectStoreTopicCatalog {
    store: Arc<dyn ObjectStore>,
    listing: Arc<dyn MaintenanceStore>,
    prefix: String,
}

impl ObjectStoreTopicCatalog {
    /// The catalog of `shard`, in `store`, paged through `listing` — which
    /// must be the same backend.
    #[must_use]
    pub fn new(
        store: Arc<dyn ObjectStore>,
        listing: Arc<dyn MaintenanceStore>,
        shard: MetadataShardId,
    ) -> Self {
        Self {
            store,
            listing,
            prefix: shard.catalog_prefix(),
        }
    }

    fn topics(&self) -> String {
        format!("{}/topic/", self.prefix)
    }

    fn tombstones(&self) -> String {
        format!("{}/tombstone/", self.prefix)
    }

    fn topic_key(&self, name: &TopicId) -> Result<ObjectKey> {
        ObjectKey::new(format!(
            "{}{}",
            self.topics(),
            hex(name.as_str().as_bytes())
        ))
    }

    fn tombstone_key(&self, name: &TopicId) -> Result<ObjectKey> {
        ObjectKey::new(format!(
            "{}{}",
            self.tombstones(),
            hex(name.as_str().as_bytes())
        ))
    }

    fn id_key(&self, id: u128) -> Result<ObjectKey> {
        ObjectKey::new(format!("{}/id/{id:032x}", self.prefix))
    }

    fn owner_prefix(&self, creator: &Principal) -> String {
        format!(
            "{}/owner/{}/",
            self.prefix,
            hex(creator.as_str().as_bytes())
        )
    }

    fn owner_key(&self, creator: &Principal, name: &TopicId) -> Result<ObjectKey> {
        ObjectKey::new(format!(
            "{}{}",
            self.owner_prefix(creator),
            hex(name.as_str().as_bytes())
        ))
    }

    /// The object under `key`, or `None` if there is none.
    async fn read(&self, key: &ObjectKey) -> Result<Option<Vec<u8>>> {
        match self.store.get(key, ByteRange::Full).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::ObjectNotFound { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }

    async fn find_entry(&self, name: &TopicId) -> Result<Option<CatalogEntry>> {
        let Some(bytes) = self.read(&self.topic_key(name)?).await? else {
            return Ok(None);
        };
        let entry = decode(&bytes)?;
        // ⚠️ An entry naming another topic is a corrupt store, not an answer.
        if entry.name() != name {
            return Err(Error::MalformedMetadataSegment { at: 5 });
        }
        Ok(Some(entry))
    }

    async fn find(&self, name: &TopicId) -> Result<Option<CatalogEntry>> {
        if self.tombstone_id(name).await?.is_some() {
            return Ok(None);
        }
        let Some(entry) = self.find_entry(name).await? else {
            return Ok(None);
        };
        let Some(id_bytes) = self.read(&self.id_key(entry.id())?).await? else {
            // An owned create publishes its topic before the id index. Keep
            // that intermediate state invisible until the index is durable.
            return Ok(None);
        };
        if id_bytes != name.as_str().as_bytes() {
            return Err(Error::MalformedMetadataSegment { at: 0 });
        }
        // The first tombstone probe only avoids unnecessary reads. A delete
        // may publish the tombstone while those reads are in flight, so the
        // final probe is the lookup's linearization check.
        if self.tombstone_id(name).await?.is_some() {
            return Ok(None);
        }
        Ok(Some(entry))
    }

    async fn find_id(&self, id: u128) -> Result<Option<CatalogEntry>> {
        let Some(bytes) = self.read(&self.id_key(id)?).await? else {
            return Ok(None);
        };
        let name = String::from_utf8(bytes)
            .ok()
            .and_then(|name| TopicId::new(name).ok())
            .ok_or(Error::MalformedMetadataSegment { at: 0 })?;
        Ok(self.find(&name).await?.filter(|entry| entry.id() == id))
    }

    async fn put_absent(&self, key: &ObjectKey, body: Vec<u8>) -> Result<bool> {
        match self
            .store
            .put(key, body, Some(Precondition::IfAbsent))
            .await
        {
            Ok(_) => Ok(true),
            Err(Error::PreconditionFailed { .. }) => Ok(false),
            Err(other) => Err(other),
        }
    }

    async fn insert(&self, name: &TopicId, partitions: u32) -> Result<CatalogEntry> {
        self.insert_with_key_domain(name, partitions, KeyDomain::default_domain())
            .await
    }

    async fn insert_with_key_domain(
        &self,
        name: &TopicId,
        partitions: u32,
        key_domain: KeyDomain,
    ) -> Result<CatalogEntry> {
        if self.tombstone_id(name).await?.is_some() {
            return Err(Error::TopicNameReserved);
        }
        let entry = CatalogEntry::with_key_domain(name.clone(), partitions, key_domain);
        if self
            .put_absent(&self.topic_key(name)?, encode(&entry)?)
            .await?
        {
            // The topic is published before its id index so a concurrent
            // delete can see and tombstone it rather than observing a bare
            // reservation.
            self.put_absent(&self.id_key(entry.id())?, name.as_str().as_bytes().to_vec())
                .await?;
            self.reject_if_tombstoned(&entry).await?;
            return Ok(entry);
        }
        // Lost the race, or it existed: the entry that is there wins.
        self.find_after_race(name).await
    }

    async fn insert_owned(
        &self,
        name: &TopicId,
        partitions: u32,
        creator: &Principal,
    ) -> Result<TopicCreateOutcome> {
        if self.tombstone_id(name).await?.is_some() {
            return Err(Error::TopicNameReserved);
        }
        let entry = CatalogEntry::with_creator(name.clone(), partitions, creator.clone());
        let body = encode(&entry)?;
        // The owner index is written first so a crash cannot publish a topic
        // whose creator is invisible after restart. Stale index keys are
        // filtered by list_owned against the topic entry. The topic entry is
        // published before the id reservation: an interruption before the
        // final id write leaves no reservation that can block a retry.
        self.put_absent(&self.owner_key(creator, name)?, Vec::new())
            .await?;
        let created = self.put_absent(&self.topic_key(name)?, body).await?;
        if !created {
            return Ok(TopicCreateOutcome {
                entry: self.find_after_race(name).await?,
                created: false,
            });
        }
        // If this final write is interrupted, the topic and owner index are
        // already durable and a retry observes the topic instead of waiting
        // on an orphan reservation.
        self.put_absent(&self.id_key(entry.id())?, name.as_str().as_bytes().to_vec())
            .await?;
        self.reject_if_tombstoned(&entry).await?;
        Ok(TopicCreateOutcome { entry, created })
    }

    async fn reject_if_tombstoned(&self, entry: &CatalogEntry) -> Result<()> {
        if self.tombstone_id(entry.name()).await?.is_some() {
            self.delete_live_indexes(entry).await?;
            return Err(Error::TopicNameReserved);
        }
        Ok(())
    }

    /// A losing caller can see a topic entry while the winner is still
    /// publishing its id index, so keep yielding until the winner's create-only
    /// write is visible. A caller that loses this second conditional write has
    /// proof that another live creator owns the name; a crashed creator cannot
    /// make this caller lose that write. A bounded probe count prevents direct
    /// catalog callers from hanging forever; `Transient` lets each caller apply
    /// its own retry policy, while the broker's `CreateTopics` handler also owns
    /// the request timeout around this wait.
    async fn find_after_race(&self, name: &TopicId) -> Result<CatalogEntry> {
        if self.tombstone_id(name).await?.is_some() {
            return Err(Error::TopicNameReserved);
        }
        for _ in 0..RACE_VISIBILITY_ATTEMPTS {
            if self.tombstone_id(name).await?.is_some() {
                return Err(Error::TopicNameReserved);
            }
            if let Some(entry) = self.find_entry(name).await? {
                self.put_absent(&self.id_key(entry.id())?, name.as_str().as_bytes().to_vec())
                    .await?;
                if self.find(name).await?.is_some() {
                    return Ok(entry);
                }
            }
            cooperative_yield().await;
        }
        Err(Error::Transient)
    }

    async fn owned_page(
        &self,
        creator: &Principal,
        after: Option<&TopicId>,
        limit: usize,
    ) -> Result<Vec<TopicId>> {
        let prefix = self.owner_prefix(creator);
        let after = after
            .map(|name| self.owner_key(creator, name))
            .transpose()?;
        let mut names = Vec::with_capacity(limit);
        let mut cursor = after;
        while Self::owned_page_needs_more(names.len(), limit) {
            let want = Self::owned_page_remaining(limit, names.len());
            let keys = self.listing.list(&prefix, cursor.as_ref(), want).await?;
            let Some(last) = keys.last().cloned() else {
                return Ok(names);
            };
            for key in keys {
                let Some(hex_name) = key.as_str().strip_prefix(&prefix) else {
                    return Err(Error::MalformedMetadataSegment { at: 0 });
                };
                let Some(bytes) = unhex(hex_name) else {
                    return Err(Error::MalformedMetadataSegment { at: prefix.len() });
                };
                let Some(text) = String::from_utf8(bytes).ok() else {
                    return Err(Error::MalformedMetadataSegment { at: prefix.len() });
                };
                let Some(name) = TopicId::new(text).ok() else {
                    return Err(Error::MalformedMetadataSegment { at: prefix.len() });
                };
                if self
                    .find(&name)
                    .await?
                    .is_some_and(|entry| entry.creator() == Some(creator))
                {
                    names.push(name);
                    if Self::owned_page_reached_limit(names.len(), limit) {
                        break;
                    }
                }
            }
            cursor = Some(last);
            if Self::owned_page_reached_limit(names.len(), limit) {
                return Ok(names);
            }
            if cursor.is_none() {
                return Ok(names);
            }
        }
        Ok(names)
    }

    const fn owned_page_needs_more(names_len: usize, limit: usize) -> bool {
        names_len < limit
    }

    const fn owned_page_remaining(limit: usize, names_len: usize) -> usize {
        limit - names_len
    }

    const fn owned_page_reached_limit(names_len: usize, limit: usize) -> bool {
        names_len == limit
    }

    /// Creates a topic in a customer key domain, or returns the existing
    /// entry if another caller won the create race.
    ///
    /// # Errors
    ///
    /// The store's error, or [`Error::MalformedMetadataSegment`] if the
    /// entry cannot be represented by the catalog format.
    pub async fn create_with_key_domain(
        &self,
        name: &TopicId,
        partitions: u32,
        key_domain: KeyDomain,
    ) -> Result<CatalogEntry> {
        self.insert_with_key_domain(name, partitions, key_domain)
            .await
    }

    async fn page(&self, after: Option<&TopicId>, limit: usize) -> Result<Vec<TopicId>> {
        let prefix = self.topics();
        let mut cursor = after.map(|name| self.topic_key(name)).transpose()?;
        let mut names = Vec::with_capacity(limit);
        while names.len() < limit {
            let requested = limit - names.len();
            let keys = self
                .listing
                .list(&prefix, cursor.as_ref(), requested)
                .await?;
            let Some(last) = keys.last().cloned() else {
                return Ok(names);
            };
            let short = keys.len() < requested;
            for key in keys {
                let name = key
                    .as_str()
                    .strip_prefix(&prefix)
                    .and_then(unhex)
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .and_then(|name| TopicId::new(name).ok())
                    .ok_or(Error::MalformedMetadataSegment { at: 0 })?;
                if self.find(&name).await?.is_some() {
                    names.push(name);
                    if names.len() == limit {
                        return Ok(names);
                    }
                }
            }
            cursor = Some(last);
            if short {
                return Ok(names);
            }
        }
        Ok(names)
    }
}

/// Gives the executor another chance to poll the creator that is publishing
/// the name entry, without depending on a particular async runtime.
async fn cooperative_yield() {
    let mut yielded = false;
    std::future::poll_fn(|cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
    .await;
}

impl TopicCatalog for ObjectStoreTopicCatalog {
    fn lookup<'a>(&'a self, name: &'a TopicId) -> BoxFuture<'a, Result<Option<CatalogEntry>>> {
        Box::pin(self.find(name))
    }

    fn lookup_id(&self, id: u128) -> BoxFuture<'_, Result<Option<CatalogEntry>>> {
        Box::pin(self.find_id(id))
    }

    fn create<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
    ) -> BoxFuture<'a, Result<CatalogEntry>> {
        Box::pin(self.insert(name, partitions))
    }

    fn create_owned<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
        creator: &'a Principal,
    ) -> BoxFuture<'a, Result<TopicCreateOutcome>> {
        Box::pin(self.insert_owned(name, partitions, creator))
    }

    fn delete<'a>(
        &'a self,
        name: &'a TopicId,
        expected_id: Option<u128>,
    ) -> BoxFuture<'a, Result<TopicDeleteOutcome>> {
        Box::pin(self.delete_topic(name, expected_id))
    }

    fn topic_retention_ms<'a>(&'a self, name: &'a TopicId) -> BoxFuture<'a, Result<Option<i64>>> {
        Box::pin(self.read_retention(name))
    }

    fn set_topic_retention_ms<'a>(
        &'a self,
        name: &'a TopicId,
        retention_ms: Option<i64>,
    ) -> BoxFuture<'a, Result<TopicRetentionUpdate>> {
        Box::pin(self.write_retention(name, retention_ms))
    }

    fn list_owned<'a>(
        &'a self,
        creator: &'a Principal,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>> {
        Box::pin(self.owned_page(creator, after, limit))
    }

    fn list<'a>(
        &'a self,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>> {
        Box::pin(self.page(after, limit))
    }
}

#[cfg(test)]
mod tests;
