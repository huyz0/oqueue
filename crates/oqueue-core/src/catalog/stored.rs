//! The topic catalog in object storage (`ADR-0049` point 3, `M7.3`).
//!
//! Two create-only objects per topic, under the shard's catalog prefix:
//!
//! - `<prefix>/topic/<hex of the name's UTF-8 bytes>` — the entry;
//! - `<prefix>/id/<32 hex digits of the id>` — the name, so an id finds it.
//!
//! ⚠️ **Hex, not the name itself**: any [`TopicId`] makes a valid key, `/`
//! included, and lowercase hex of the bytes sorts exactly as the bytes do, so
//! a key listing *is* name order.

use std::fmt::Write as _;
use std::sync::Arc;

use super::{CatalogEntry, TopicCatalog};
use crate::{
    BoxFuture, ByteRange, Error, MaintenanceStore, MetadataShardId, ObjectKey, ObjectStore,
    Precondition, Result, TopicId,
};

/// The entry format's version, the body's first byte.
const FORMAT: u8 = 1;

/// A [`TopicCatalog`] over object storage, one shard's worth.
///
/// ⚠️ **Creation provisions nothing** (doc 15 §2): `create` writes exactly the
/// two objects above and starts no task, timer or log. A topic costs its
/// catalog entry until something writes to it.
///
/// ⚠️ **The id key is written first**, so a topic whose entry exists has its
/// id key (guarantee 3). A crash between the two leaves an id key alone, which
/// `lookup_id` answers `None` for — what `lookup` answers.
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

    fn topic_key(&self, name: &TopicId) -> Result<ObjectKey> {
        ObjectKey::new(format!(
            "{}{}",
            self.topics(),
            hex(name.as_str().as_bytes())
        ))
    }

    fn id_key(&self, id: u128) -> Result<ObjectKey> {
        ObjectKey::new(format!("{}/id/{id:032x}", self.prefix))
    }

    /// The object under `key`, or `None` if there is none.
    async fn read(&self, key: &ObjectKey) -> Result<Option<Vec<u8>>> {
        match self.store.get(key, ByteRange::Full).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::ObjectNotFound { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }

    async fn find(&self, name: &TopicId) -> Result<Option<CatalogEntry>> {
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
        let entry = CatalogEntry::new(name.clone(), partitions);
        // ⚠️ A present id key holds these same bytes: the id is the name's.
        self.put_absent(&self.id_key(entry.id())?, name.as_str().as_bytes().to_vec())
            .await?;
        if self
            .put_absent(&self.topic_key(name)?, encode(&entry))
            .await?
        {
            return Ok(entry);
        }
        // Lost the race, or it existed: the entry that is there wins.
        self.find(name)
            .await?
            .ok_or(Error::MalformedMetadataSegment { at: 0 })
    }

    async fn page(&self, after: Option<&TopicId>, limit: usize) -> Result<Vec<TopicId>> {
        let prefix = self.topics();
        let after = after.map(|name| self.topic_key(name)).transpose()?;
        let keys = self.listing.list(&prefix, after.as_ref(), limit).await?;
        keys.iter()
            .map(|key| {
                key.as_str()
                    .strip_prefix(&prefix)
                    .and_then(unhex)
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .and_then(|name| TopicId::new(name).ok())
                    .ok_or(Error::MalformedMetadataSegment { at: 0 })
            })
            .collect()
    }
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

    fn list<'a>(
        &'a self,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>> {
        Box::pin(self.page(after, limit))
    }
}

/// `FORMAT`, the partition count big-endian, then the name's bytes.
fn encode(entry: &CatalogEntry) -> Vec<u8> {
    let mut out = vec![FORMAT];
    out.extend_from_slice(&entry.partitions().to_be_bytes());
    out.extend_from_slice(entry.name().as_str().as_bytes());
    out
}

/// ⚠️ **Refused whole, never misread**: an unknown format, a short body, or a
/// name that is not a topic's is [`Error::MalformedMetadataSegment`].
fn decode(bytes: &[u8]) -> Result<CatalogEntry> {
    if bytes.first() != Some(&FORMAT) {
        return Err(Error::MalformedMetadataSegment { at: 0 });
    }
    let partitions: [u8; 4] = bytes
        .get(1..5)
        .and_then(|raw| raw.try_into().ok())
        .ok_or(Error::MalformedMetadataSegment { at: 1 })?;
    let name = bytes
        .get(5..)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|name| TopicId::new(name).ok())
        .ok_or(Error::MalformedMetadataSegment { at: 5 })?;
    Ok(CatalogEntry::new(name, u32::from_be_bytes(partitions)))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// ⚠️ Lowercase only, as [`hex`] writes: anything else is not one of ours.
fn unhex(text: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let raw = text.as_bytes();
    if !raw.len().is_multiple_of(2) {
        return None;
    }
    raw.chunks_exact(2)
        .map(|pair| match pair {
            [high, low] => Some(digit(*high)? << 4 | digit(*low)?),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
