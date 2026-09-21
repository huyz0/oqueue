//! The topic catalog seam (`ADR-0049` point 2): what topics exist, looked up
//! one at a time, so no node has to hold the whole catalog.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use crate::{BoxFuture, KeyDomain, Principal, Result, TopicId};

mod stored;

pub use stored::ObjectStoreTopicCatalog;

/// One topic's catalog entry: its name, its id, partition count, key domain,
/// and optional durable creator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    name: TopicId,
    id: u128,
    partitions: u32,
    key_domain: KeyDomain,
    creator: Option<Principal>,
}

impl CatalogEntry {
    /// The entry for `name` with `partitions` partitions, its id derived from
    /// the name ([`topic_uuid`]).
    #[must_use]
    pub fn new(name: TopicId, partitions: u32) -> Self {
        let id = topic_uuid(&name);
        Self {
            name,
            id,
            partitions,
            key_domain: KeyDomain::default_domain(),
            creator: None,
        }
    }

    /// The entry for `name` in a specific key domain.
    #[must_use]
    pub fn with_key_domain(name: TopicId, partitions: u32, key_domain: KeyDomain) -> Self {
        let id = topic_uuid(&name);
        Self {
            name,
            id,
            partitions,
            key_domain,
            creator: None,
        }
    }

    /// The entry for a topic created by `creator`.
    #[must_use]
    pub fn with_creator(name: TopicId, partitions: u32, creator: Principal) -> Self {
        let mut entry = Self::new(name, partitions);
        entry.creator = Some(creator);
        entry
    }

    /// The entry for a customer-key topic created by `creator`.
    #[must_use]
    pub fn with_key_domain_and_creator(
        name: TopicId,
        partitions: u32,
        key_domain: KeyDomain,
        creator: Principal,
    ) -> Self {
        let mut entry = Self::with_key_domain(name, partitions, key_domain);
        entry.creator = Some(creator);
        entry
    }

    /// The topic's name.
    #[must_use]
    pub const fn name(&self) -> &TopicId {
        &self.name
    }

    /// The topic's id, as the 128 bits of a Kafka topic UUID.
    #[must_use]
    pub const fn id(&self) -> u128 {
        self.id
    }

    /// How many partitions the topic has.
    #[must_use]
    pub const fn partitions(&self) -> u32 {
        self.partitions
    }

    /// The topic's authoritative key domain.
    #[must_use]
    pub const fn key_domain(&self) -> &KeyDomain {
        &self.key_domain
    }

    /// The principal that durably created this topic, if any.
    #[must_use]
    pub const fn creator(&self) -> Option<&Principal> {
        self.creator.as_ref()
    }
}

/// The result of an owned create operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicCreateOutcome {
    entry: CatalogEntry,
    created: bool,
}

impl TopicCreateOutcome {
    /// Builds a create result for an alternative catalog implementation.
    #[must_use]
    pub const fn new(entry: CatalogEntry, created: bool) -> Self {
        Self { entry, created }
    }

    /// The entry that exists after the operation.
    #[must_use]
    pub const fn entry(&self) -> &CatalogEntry {
        &self.entry
    }

    /// Whether this caller won the conditional create.
    #[must_use]
    pub const fn created(&self) -> bool {
        self.created
    }
}

/// A topic's id, derived from its name (`ADR-0049` point 3).
///
/// ⚠️ **Derived, so two nodes creating one topic agree without talking**, and
/// never nil — nil is the wire's "no id". ⚠️ **A deleted-and-recreated topic
/// would reuse it**, which KIP-516 clients read as the same topic; deletion
/// does not exist yet, and whoever builds it changes this.
///
/// FNV-1a over the name's bytes, 128-bit, with the UUID version nibble set to
/// 8 (custom) and the RFC 4122 variant bits.
#[must_use]
pub fn topic_uuid(name: &TopicId) -> u128 {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    for byte in name.as_str().bytes() {
        hash ^= u128::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    let version = 0x8_u128 << 76;
    let variant = 0x2_u128 << 62;
    (hash & !(0xf_u128 << 76) & !(0x3_u128 << 62)) | version | variant
}

/// What topics exist.
///
/// # What an implementor must guarantee
///
/// 1. **`create` is idempotent and race-free**: of any number of callers
///    creating one name, all get the same entry, and a name that exists is
///    returned as it is — never overwritten with a different partition count.
/// 2. **`create` provisions nothing** (`ADR-0049` point 3, doc 15 §2): it
///    records the entry and starts no task, timer, log or coordinator.
/// 3. **`lookup_id` answers what `lookup` answers**, for every created topic.
/// 4. **`list` pages in name order**: names strictly after `after`, at most
///    `limit` of them, so a caller can bound what one answer costs.
pub trait TopicCatalog: Send + Sync + fmt::Debug {
    /// The entry for `name`, or `None` if it does not exist.
    fn lookup<'a>(&'a self, name: &'a TopicId) -> BoxFuture<'a, Result<Option<CatalogEntry>>>;

    /// The entry whose id is `id`, or `None`.
    fn lookup_id(&self, id: u128) -> BoxFuture<'_, Result<Option<CatalogEntry>>>;

    /// Creates `name` with `partitions` partitions if absent; the entry that
    /// exists afterwards either way.
    fn create<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
    ) -> BoxFuture<'a, Result<CatalogEntry>>;

    /// Creates `name` on behalf of `creator`, returning whether this caller
    /// won the conditional create. A losing caller receives the existing
    /// entry and never changes its creator.
    fn create_owned<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
        creator: &'a Principal,
    ) -> BoxFuture<'a, Result<TopicCreateOutcome>>;

    /// Up to `limit` topics durably owned by `creator`, after `after`.
    fn list_owned<'a>(
        &'a self,
        creator: &'a Principal,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>>;

    /// Up to `limit` topic names after `after`, in name order.
    fn list<'a>(
        &'a self,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>>;
}

/// An in-memory [`TopicCatalog`].
#[derive(Debug, Default)]
pub struct FakeTopicCatalog {
    inner: Mutex<Entries>,
}

#[derive(Debug, Default)]
struct Entries {
    by_name: BTreeMap<TopicId, CatalogEntry>,
    by_id: HashMap<u128, TopicId>,
}

impl FakeTopicCatalog {
    /// An empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates `name` in `key_domain`, or returns the existing entry.
    ///
    /// This is the in-memory catalog's configuration hook for BYOK tests. The
    /// object-store catalog exposes the same operation as an inherent async
    /// method because the trait's default `create` contract remains the
    /// provider-managed path until the broker's topic-creation API grows a
    /// key-domain field.
    pub fn create_with_key_domain(
        &self,
        name: &TopicId,
        partitions: u32,
        key_domain: KeyDomain,
    ) -> CatalogEntry {
        self.with(|e| {
            if let Some(existing) = e.by_name.get(name) {
                return existing.clone();
            }
            let entry = CatalogEntry::with_key_domain(name.clone(), partitions, key_domain);
            e.by_id.insert(entry.id(), name.clone());
            e.by_name.insert(name.clone(), entry.clone());
            entry
        })
    }

    fn with<T>(&self, f: impl FnOnce(&mut Entries) -> T) -> T {
        f(&mut self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

impl TopicCatalog for FakeTopicCatalog {
    fn lookup<'a>(&'a self, name: &'a TopicId) -> BoxFuture<'a, Result<Option<CatalogEntry>>> {
        Box::pin(async move { Ok(self.with(|e| e.by_name.get(name).cloned())) })
    }

    fn lookup_id(&self, id: u128) -> BoxFuture<'_, Result<Option<CatalogEntry>>> {
        Box::pin(async move {
            Ok(self.with(|e| {
                e.by_id
                    .get(&id)
                    .and_then(|name| e.by_name.get(name))
                    .cloned()
            }))
        })
    }

    fn create<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
    ) -> BoxFuture<'a, Result<CatalogEntry>> {
        Box::pin(async move {
            Ok(self.create_with_key_domain(name, partitions, KeyDomain::default_domain()))
        })
    }

    fn create_owned<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
        creator: &'a Principal,
    ) -> BoxFuture<'a, Result<TopicCreateOutcome>> {
        Box::pin(async move {
            Ok(self.with(|e| {
                if let Some(entry) = e.by_name.get(name) {
                    return TopicCreateOutcome {
                        entry: entry.clone(),
                        created: false,
                    };
                }
                let entry = CatalogEntry::with_creator(name.clone(), partitions, creator.clone());
                e.by_id.insert(entry.id(), name.clone());
                e.by_name.insert(name.clone(), entry.clone());
                TopicCreateOutcome {
                    entry,
                    created: true,
                }
            }))
        })
    }

    fn list_owned<'a>(
        &'a self,
        creator: &'a Principal,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>> {
        Box::pin(async move {
            Ok(self.with(|e| {
                let from = after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
                e.by_name
                    .range((from, std::ops::Bound::Unbounded))
                    .filter(|(_, entry)| entry.creator() == Some(creator))
                    .take(limit)
                    .map(|(name, _)| name.clone())
                    .collect()
            }))
        })
    }

    fn list<'a>(
        &'a self,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>> {
        Box::pin(async move {
            Ok(self.with(|e| {
                let from = after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
                e.by_name
                    .range((from, std::ops::Bound::Unbounded))
                    .take(limit)
                    .map(|(name, _)| name.clone())
                    .collect()
            }))
        })
    }
}

#[cfg(test)]
mod tests;
