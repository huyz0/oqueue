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
    BoxFuture, ByteRange, Error, KeyDomain, MaintenanceStore, MetadataShardId, ObjectKey,
    ObjectStore, Precondition, Result, TopicId,
};

/// The entry format's version, the body's first byte.
const DEFAULT_FORMAT: u8 = 1;
const CUSTOMER_FORMAT: u8 = 2;

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
        self.insert_with_key_domain(name, partitions, KeyDomain::default_domain())
            .await
    }

    async fn insert_with_key_domain(
        &self,
        name: &TopicId,
        partitions: u32,
        key_domain: KeyDomain,
    ) -> Result<CatalogEntry> {
        let entry = CatalogEntry::with_key_domain(name.clone(), partitions, key_domain);
        // ⚠️ A present id key holds these same bytes: the id is the name's.
        self.put_absent(&self.id_key(entry.id())?, name.as_str().as_bytes().to_vec())
            .await?;
        if self
            .put_absent(&self.topic_key(name)?, encode(&entry)?)
            .await?
        {
            return Ok(entry);
        }
        // Lost the race, or it existed: the entry that is there wins.
        self.find(name)
            .await?
            .ok_or(Error::MalformedMetadataSegment { at: 0 })
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

/// The default format remains byte-identical: `DEFAULT_FORMAT`, partition
/// count big-endian, then the name's bytes. Customer entries use a v2 shape
/// with explicit name and key-id lengths so both metadata fields can be read
/// back without guessing a boundary.
fn encode(entry: &CatalogEntry) -> Result<Vec<u8>> {
    let name = entry.name().as_str().as_bytes();
    match entry.key_domain() {
        KeyDomain::Default => {
            let mut out = vec![DEFAULT_FORMAT];
            out.extend_from_slice(&entry.partitions().to_be_bytes());
            out.extend_from_slice(name);
            Ok(out)
        }
        KeyDomain::Customer(key_id) => {
            let key = key_id.as_str().as_bytes();
            let mut out = vec![CUSTOMER_FORMAT];
            out.extend_from_slice(&entry.partitions().to_be_bytes());
            let name_len =
                u16::try_from(name.len()).map_err(|_| Error::MalformedMetadataSegment { at: 5 })?;
            out.extend_from_slice(&name_len.to_be_bytes());
            out.extend_from_slice(name);
            let key_len = u16::try_from(key.len())
                .map_err(|_| Error::MalformedMetadataSegment { at: 7 + name.len() })?;
            out.extend_from_slice(&key_len.to_be_bytes());
            out.extend_from_slice(key);
            Ok(out)
        }
    }
}

/// ⚠️ **Refused whole, never misread**: an unknown format, a short body, or a
/// name that is not a topic's is [`Error::MalformedMetadataSegment`].
fn decode(bytes: &[u8]) -> Result<CatalogEntry> {
    let format = *bytes
        .first()
        .ok_or(Error::MalformedMetadataSegment { at: 0 })?;
    let partitions: [u8; 4] = bytes
        .get(1..5)
        .and_then(|raw| raw.try_into().ok())
        .ok_or(Error::MalformedMetadataSegment { at: 1 })?;
    let partitions = u32::from_be_bytes(partitions);
    match format {
        DEFAULT_FORMAT => decode_default(bytes, partitions),
        CUSTOMER_FORMAT => decode_customer(bytes, partitions),
        _ => Err(Error::MalformedMetadataSegment { at: 0 }),
    }
}

fn decode_default(bytes: &[u8], partitions: u32) -> Result<CatalogEntry> {
    let name = bytes
        .get(5..)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|name| TopicId::new(name).ok())
        .ok_or(Error::MalformedMetadataSegment { at: 5 })?;
    Ok(CatalogEntry::new(name, partitions))
}

fn decode_customer(bytes: &[u8], partitions: u32) -> Result<CatalogEntry> {
    let name_len = bytes
        .get(5..7)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: 5 })?;
    let name_len = usize::from(name_len);
    let name_end = 7usize
        .checked_add(name_len)
        .ok_or(Error::MalformedMetadataSegment { at: 7 })?;
    let name = bytes
        .get(7..name_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|name| TopicId::new(name).ok())
        .ok_or(Error::MalformedMetadataSegment { at: 7 })?;
    let key_len_at = name_end;
    let key_len = bytes
        .get(key_len_at..key_len_at + 2)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: key_len_at })?;
    let key_start = key_len_at + 2;
    let key_end = key_start
        .checked_add(usize::from(key_len))
        .ok_or(Error::MalformedMetadataSegment { at: key_start })?;
    let key = bytes
        .get(key_start..key_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|key| crate::KeyId::new(key).ok())
        .ok_or(Error::MalformedMetadataSegment { at: key_start })?;
    if key_end != bytes.len() {
        return Err(Error::MalformedMetadataSegment { at: key_end });
    }
    Ok(CatalogEntry::with_key_domain(
        name,
        partitions,
        KeyDomain::customer(key),
    ))
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
