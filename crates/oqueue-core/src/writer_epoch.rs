//! Durable allocation of nonce writer epochs.
//!
//! A coordinator fence identifies one coordinator incarnation, not one writer
//! process. Several writers can therefore share it. Writer epochs use a
//! separate immutable create-only counter so every process and restart gets a
//! distinct value before it can construct a [`NonceMinter`](crate::NonceMinter).

use crate::{
    ByteRange, Error, MAX_WRITER_EPOCH, MaintenanceStore, ObjectKey, ObjectStore, Precondition,
    Result, WriterEpoch,
};
use std::collections::BTreeSet;
use std::sync::Arc;

const TERM_MAGIC: &[u8] = b"OQWE";
const MARKER_MAGIC: &[u8] = b"OQWE-MARKER";
const LIST_PAGE: usize = 1_000;
const MAX_ALLOCATION_ATTEMPTS: usize = 128;

/// Allocates writer epochs from immutable object-store terms.
pub struct DurableWriterEpochAllocator {
    store: Arc<dyn ObjectStore>,
    maintenance: Arc<dyn MaintenanceStore>,
    prefix: String,
}

impl core::fmt::Debug for DurableWriterEpochAllocator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DurableWriterEpochAllocator")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl DurableWriterEpochAllocator {
    /// Creates an allocator whose terms live below `prefix`.
    #[must_use]
    pub fn new(
        store: Arc<dyn ObjectStore>,
        maintenance: Arc<dyn MaintenanceStore>,
        prefix: impl Into<String>,
    ) -> Self {
        Self {
            store,
            maintenance,
            prefix: prefix.into(),
        }
    }

    /// Allocates the next writer epoch.
    ///
    /// Each term is an immutable object written with `IfAbsent`. If several
    /// allocators observe the same newest term, only one can create the next
    /// object; the others observe the conditional-write failure and retry from
    /// the now-advanced durable counter.
    ///
    /// # Errors
    ///
    /// Returns the object-store error, malformed epoch metadata, or
    /// [`Error::NonceWriterEpochOutOfRange`] when the 40-bit nonce field is
    /// exhausted.
    pub async fn allocate(&self) -> Result<WriterEpoch> {
        for _ in 0..MAX_ALLOCATION_ATTEMPTS {
            let newest = self.newest().await?;
            if newest.is_none() {
                if self.marker_exists().await? {
                    return Err(Error::MalformedMetadataSegment { at: 0 });
                }
                match self
                    .store
                    .put(
                        &self.key(0)?,
                        TERM_MAGIC.to_vec(),
                        Some(Precondition::IfAbsent),
                    )
                    .await
                {
                    Ok(_) => {
                        self.ensure_marker().await?;
                        return Ok(WriterEpoch::from_durable_counter(0));
                    }
                    Err(Error::PreconditionFailed { .. }) => continue,
                    Err(other) => return Err(other),
                }
            }

            // The marker is created before any term after zero. If term zero
            // is later absent while this marker remains, the history is
            // corrupt rather than an invitation to reuse epoch zero.
            self.ensure_marker().await?;
            let Some(newest) = newest else {
                return Err(Error::MalformedMetadataSegment { at: 0 });
            };
            let next = next_term(newest)?;
            match self
                .store
                .put(
                    &self.key(next)?,
                    TERM_MAGIC.to_vec(),
                    Some(Precondition::IfAbsent),
                )
                .await
            {
                Ok(_) => return Ok(WriterEpoch::from_durable_counter(next)),
                Err(Error::PreconditionFailed { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        Err(Error::Transient)
    }

    async fn read_term(&self, term: u64) -> Result<Option<()>> {
        let bytes = match self.store.get(&self.key(term)?, ByteRange::Full).await {
            Ok(bytes) => bytes,
            Err(Error::ObjectNotFound { .. }) => return Ok(None),
            Err(other) => return Err(other),
        };
        if bytes != TERM_MAGIC {
            return Err(Error::MalformedMetadataSegment { at: 0 });
        }
        Ok(Some(()))
    }

    async fn marker_exists(&self) -> Result<bool> {
        match self.store.get(&self.marker_key()?, ByteRange::Full).await {
            Ok(bytes) if bytes == MARKER_MAGIC => Ok(true),
            Ok(_) => Err(Error::MalformedMetadataSegment { at: 0 }),
            Err(Error::ObjectNotFound { .. }) => Ok(false),
            Err(other) => Err(other),
        }
    }

    async fn ensure_marker(&self) -> Result<()> {
        match self
            .store
            .put(
                &self.marker_key()?,
                MARKER_MAGIC.to_vec(),
                Some(Precondition::IfAbsent),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(Error::PreconditionFailed { .. }) => {
                if self.marker_exists().await? {
                    Ok(())
                } else {
                    Err(Error::MalformedMetadataSegment { at: 0 })
                }
            }
            Err(other) => Err(other),
        }
    }

    async fn newest(&self) -> Result<Option<u64>> {
        let terms = self.list_terms().await?;
        let Some(newest) = terms.iter().next_back().copied() else {
            return Ok(None);
        };
        for term in &terms {
            if self.read_term(*term).await?.is_none() {
                return Err(Error::MalformedMetadataSegment { at: 0 });
            }
        }
        Ok(Some(newest))
    }

    async fn list_terms(&self) -> Result<BTreeSet<u64>> {
        let prefix = format!("{}/writer-epoch/", self.prefix);
        let mut terms = BTreeSet::new();
        let mut after = None;
        loop {
            let page = self
                .maintenance
                .list(&prefix, after.as_ref(), LIST_PAGE)
                .await?;
            let Some(last) = page.last().cloned() else {
                break;
            };
            for key in &page {
                let suffix = key
                    .as_str()
                    .strip_prefix(&prefix)
                    .ok_or(Error::MalformedMetadataSegment { at: 0 })?;
                if suffix == "marker" {
                    continue;
                }
                if suffix.len() != 20 || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(Error::MalformedMetadataSegment { at: 0 });
                }
                let term = suffix
                    .parse::<u64>()
                    .map_err(|_| Error::MalformedMetadataSegment { at: 0 })?;
                terms.insert(term);
            }
            after = Some(last);
            if page.len() == LIST_PAGE {
                continue;
            }
            break;
        }
        let mut expected = 0_u64;
        for term in &terms {
            if *term != expected {
                return Err(Error::MalformedMetadataSegment { at: 0 });
            }
            expected = expected
                .checked_add(1)
                .ok_or(Error::NonceWriterEpochOutOfRange { got: u64::MAX })?;
        }
        Ok(terms)
    }

    fn key(&self, term: u64) -> Result<ObjectKey> {
        ObjectKey::new(format!("{}/writer-epoch/{term:020}", self.prefix))
    }

    fn marker_key(&self) -> Result<ObjectKey> {
        ObjectKey::new(format!("{}/writer-epoch/marker", self.prefix))
    }
}

fn next_term(newest: u64) -> Result<u64> {
    let next = newest
        .checked_add(1)
        .ok_or(Error::NonceWriterEpochOutOfRange { got: u64::MAX })?;
    if next > MAX_WRITER_EPOCH {
        return Err(Error::NonceWriterEpochOutOfRange { got: next });
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{DurableWriterEpochAllocator, MAX_WRITER_EPOCH, next_term};
    use crate::{
        Error, FakeObjectStore, MaintenanceStore, ObjectKey, ObjectStore, Precondition,
        test_executor::block_on,
    };
    use std::sync::Arc;

    fn allocator() -> (DurableWriterEpochAllocator, Arc<FakeObjectStore>) {
        let store = Arc::new(FakeObjectStore::new());
        let object_store: Arc<dyn ObjectStore> = Arc::clone(&store) as _;
        let maintenance: Arc<dyn MaintenanceStore> = Arc::clone(&store) as _;
        (
            DurableWriterEpochAllocator::new(object_store, maintenance, "meta/0"),
            store,
        )
    }

    #[test]
    fn the_epoch_accessor_returns_the_durable_counter() {
        assert_eq!(crate::WriterEpoch::from_durable_counter(7).get(), 7);
    }

    #[test]
    fn the_allocator_debug_name_includes_only_its_prefix() {
        let (allocator, _) = allocator();
        let debug = format!("{allocator:?}");
        assert!(debug.contains("DurableWriterEpochAllocator"));
        assert!(debug.contains("meta/0"));
    }

    #[test]
    fn the_maximum_epoch_is_allocatable_but_the_next_one_is_not() {
        assert_eq!(
            next_term(MAX_WRITER_EPOCH - 1).expect("maximum fits"),
            MAX_WRITER_EPOCH
        );
        assert_eq!(
            next_term(MAX_WRITER_EPOCH),
            Err(Error::NonceWriterEpochOutOfRange {
                got: MAX_WRITER_EPOCH + 1
            })
        );
    }

    #[test]
    fn malformed_term_payload_is_refused() {
        let (allocator, store) = allocator();
        block_on(store.put(
            &allocator.key(0).expect("term key"),
            b"corrupt".to_vec(),
            Some(Precondition::IfAbsent),
        ))
        .expect("term is installed");
        assert_eq!(
            block_on(allocator.read_term(0)),
            Err(Error::MalformedMetadataSegment { at: 0 })
        );
    }

    #[test]
    fn malformed_marker_is_refused_by_both_marker_checks() {
        let (allocator, store) = allocator();
        block_on(store.put(
            &allocator.marker_key().expect("marker key"),
            b"corrupt".to_vec(),
            Some(Precondition::IfAbsent),
        ))
        .expect("marker is installed");
        assert_eq!(
            block_on(allocator.marker_exists()),
            Err(Error::MalformedMetadataSegment { at: 0 })
        );
        assert_eq!(
            block_on(allocator.ensure_marker()),
            Err(Error::MalformedMetadataSegment { at: 0 })
        );
    }

    #[test]
    fn a_term_name_with_the_wrong_length_is_refused() {
        let (allocator, store) = allocator();
        let key = ObjectKey::new("meta/0/writer-epoch/0000000000000000000")
            .expect("the malformed name is still a valid object key");
        block_on(store.put(&key, b"OQWE".to_vec(), None)).expect("term is installed");
        assert_eq!(
            block_on(allocator.list_terms()),
            Err(Error::MalformedMetadataSegment { at: 0 })
        );
    }

    #[test]
    fn listing_continues_after_an_exactly_full_page() {
        let (allocator, store) = allocator();
        for term in 0..=1_000 {
            block_on(store.put(
                &allocator.key(term).expect("term key"),
                b"OQWE".to_vec(),
                Some(Precondition::IfAbsent),
            ))
            .expect("term is installed");
        }
        let terms = block_on(allocator.list_terms()).expect("all terms are listed");
        assert_eq!(terms.len(), 1_001);
        assert_eq!(terms.iter().next_back(), Some(&1_000));
    }
}
