//! A [`TopicCatalog`] of any size that holds none of it (`ADR-0049` point 5).
//!
//! Topic `n` for every `n < size` is named `t{n:08}` and has one partition.
//! Each entry is *derived* on lookup — parsed out of the name — so the fixture
//! is a `u64` whatever its size, and a scale test measuring the node measures
//! the node rather than the fixture.
//!
//! ⚠️ **What it cannot do, and why the scale tests do not need it:**
//! - `lookup_id` answers `None`. An id is a hash of the name
//!   (`oqueue_core::topic_uuid`) and cannot be inverted without holding every
//!   name, which is the thing this fixture exists not to do. The node only
//!   asks on a *cache miss*; a topic it resolved by name first is answered
//!   from its own cache, which is how the scale tests reach id-addressed
//!   `Produce` v13. A request addressing an id the node never resolved by
//!   name is outside this fixture.
//! - `create` stores nothing. For a synthetic name it returns that name's
//!   entry (rule 1: an existing name is returned as it is); for any other it
//!   returns the entry asked for without recording it, so a later `lookup`
//!   says the topic does not exist. No scale test creates a topic.

use oqueue_core::{BoxFuture, CatalogEntry, Result, TopicCatalog, TopicId};

/// Width of the index in a synthetic name: enough for 10^8 topics, and fixed
/// so name order and index order agree.
const WIDTH: usize = 8;

/// A catalog of `size` topics, derived rather than stored.
#[derive(Debug, Clone, Copy)]
pub struct SyntheticCatalog {
    size: u64,
}

impl SyntheticCatalog {
    /// `size` topics, `t00000000` onwards.
    pub const fn new(size: u64) -> Self {
        Self { size }
    }

    /// The name of topic `index`.
    pub fn name(index: u64) -> String {
        format!("t{index:0WIDTH$}")
    }

    /// The index `name` denotes, if it is a topic of this catalog.
    fn index_of(self, name: &str) -> Option<u64> {
        let digits = name.strip_prefix('t')?;
        if digits.len() != WIDTH || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok().filter(|&n| n < self.size)
    }

    fn entry(index: u64) -> CatalogEntry {
        CatalogEntry::new(
            TopicId::new(Self::name(index)).expect("a synthetic name is valid"),
            1,
        )
    }

    /// The first index whose name sorts strictly after `after`: a binary
    /// search over names, O(log size), since fixed width makes name order
    /// index order.
    fn first_after(self, after: &str) -> u64 {
        let (mut lo, mut hi) = (0, self.size);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if Self::name(mid).as_str() > after {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    }
}

impl TopicCatalog for SyntheticCatalog {
    fn lookup<'a>(&'a self, name: &'a TopicId) -> BoxFuture<'a, Result<Option<CatalogEntry>>> {
        Box::pin(async move { Ok(self.index_of(name.as_str()).map(Self::entry)) })
    }

    fn lookup_id(&self, _id: u128) -> BoxFuture<'_, Result<Option<CatalogEntry>>> {
        // Not invertible without holding every name: see the module doc.
        Box::pin(async { Ok(None) })
    }

    fn create<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
    ) -> BoxFuture<'a, Result<CatalogEntry>> {
        Box::pin(async move {
            Ok(self
                .index_of(name.as_str())
                .map_or_else(|| CatalogEntry::new(name.clone(), partitions), Self::entry))
        })
    }

    fn list<'a>(
        &'a self,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>> {
        Box::pin(async move {
            let start = after.map_or(0, |a| self.first_after(a.as_str()));
            let end = self
                .size
                .min(start.saturating_add(u64::try_from(limit).unwrap_or(u64::MAX)));
            Ok((start..end)
                .map(|n| TopicId::new(Self::name(n)).expect("a synthetic name is valid"))
                .collect())
        })
    }
}

#[tokio::test]
async fn the_synthetic_catalog_answers_like_a_catalog() {
    let _serial = crate::serial().await;
    let catalog = SyntheticCatalog::new(10);
    let name = |s: &str| TopicId::new(s).expect("valid");
    let found = catalog.lookup(&name("t00000009")).await.expect("answers");
    assert_eq!(found.map(|e| e.partitions()), Some(1));
    assert!(
        catalog
            .lookup(&name("t00000010"))
            .await
            .expect("answers")
            .is_none()
    );
    assert!(
        catalog
            .lookup(&name("t0000001"))
            .await
            .expect("answers")
            .is_none()
    );
    let page = catalog
        .list(Some(&name("t00000007")), 5)
        .await
        .expect("pages");
    assert_eq!(page, vec![name("t00000008"), name("t00000009")]);
    let first = catalog.list(Some(&name("a")), 1).await.expect("pages");
    assert_eq!(first, vec![name("t00000000")]);
}

/// Every call a node makes on a [`TopicCatalog`], counted.
///
/// ⚠️ **Calls, not names returned** (the review finding on `M7.4`):
/// `Cluster::topic_lookups` counts names a listing returned, so it cannot see
/// a node that pages the whole catalog and keeps the first page. `listed`
/// counts every name any `list` call produced, which it can.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Calls {
    pub lookup: u64,
    pub lookup_id: u64,
    pub create: u64,
    pub list: u64,
    pub listed: u64,
}

/// `C`, with every call counted.
#[derive(Debug)]
pub struct Counted<C> {
    inner: C,
    calls: std::sync::Mutex<Calls>,
}

impl<C> Counted<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            calls: std::sync::Mutex::default(),
        }
    }

    /// The calls counted so far.
    pub fn calls(&self) -> Calls {
        *self.count()
    }

    fn count(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<C: TopicCatalog> TopicCatalog for Counted<C> {
    fn lookup<'a>(&'a self, name: &'a TopicId) -> BoxFuture<'a, Result<Option<CatalogEntry>>> {
        self.count().lookup += 1;
        self.inner.lookup(name)
    }

    fn lookup_id(&self, id: u128) -> BoxFuture<'_, Result<Option<CatalogEntry>>> {
        self.count().lookup_id += 1;
        self.inner.lookup_id(id)
    }

    fn create<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
    ) -> BoxFuture<'a, Result<CatalogEntry>> {
        self.count().create += 1;
        self.inner.create(name, partitions)
    }

    fn list<'a>(
        &'a self,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<TopicId>>> {
        self.count().list += 1;
        Box::pin(async move {
            let page = self.inner.list(after, limit).await?;
            self.count().listed += u64::try_from(page.len()).unwrap_or(u64::MAX);
            Ok(page)
        })
    }
}
