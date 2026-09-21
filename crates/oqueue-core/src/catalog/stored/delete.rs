use super::ObjectStoreTopicCatalog;
use super::format::encode;
use crate::{CatalogEntry, Error, Result, TopicDeleteOutcome, TopicId};

impl ObjectStoreTopicCatalog {
    async fn tombstone_entry(&self, name: &TopicId) -> Result<Option<CatalogEntry>> {
        let Some(bytes) = self.read(&self.tombstone_key(name)?).await? else {
            return Ok(None);
        };
        let entry = super::decode(&bytes)?;
        if entry.name() != name {
            return Err(Error::MalformedMetadataSegment { at: 0 });
        }
        Ok(Some(entry))
    }

    pub(super) async fn tombstone_id(&self, name: &TopicId) -> Result<Option<u128>> {
        Ok(self.tombstone_entry(name).await?.map(|entry| entry.id()))
    }

    pub(super) async fn delete_topic(
        &self,
        name: &TopicId,
        expected_id: Option<u128>,
    ) -> Result<TopicDeleteOutcome> {
        if let Some(tombstone) = self.tombstone_entry(name).await? {
            if let Some(expected) = expected_id
                && expected != tombstone.id()
            {
                return Ok(TopicDeleteOutcome::StaleId {
                    expected,
                    actual: tombstone.id(),
                });
            }
            self.delete_live_indexes(&tombstone).await?;
            return Ok(TopicDeleteOutcome::AlreadyDeleted { id: tombstone.id() });
        }
        let Some(entry) = self.find_entry(name).await? else {
            return Ok(TopicDeleteOutcome::Missing);
        };
        if let Some(expected) = expected_id
            && expected != entry.id()
        {
            return Ok(TopicDeleteOutcome::StaleId {
                expected,
                actual: entry.id(),
            });
        }
        if !self
            .put_absent(&self.tombstone_key(name)?, encode(&entry)?)
            .await?
        {
            let tombstone = self.tombstone_entry(name).await?.ok_or(Error::Transient)?;
            if let Some(expected) = expected_id
                && expected != tombstone.id()
            {
                return Ok(TopicDeleteOutcome::StaleId {
                    expected,
                    actual: tombstone.id(),
                });
            }
            self.delete_live_indexes(&tombstone).await?;
            return Ok(TopicDeleteOutcome::AlreadyDeleted { id: tombstone.id() });
        }
        self.delete_live_indexes(&entry).await?;
        Ok(TopicDeleteOutcome::Deleted(entry))
    }

    pub(super) async fn delete_live_indexes(&self, entry: &CatalogEntry) -> Result<()> {
        let name = entry.name();
        let mut keys = vec![
            self.topic_key(name)?,
            self.id_key(entry.id())?,
            self.retention_key(name)?,
        ];
        if let Some(creator) = entry.creator() {
            keys.push(self.owner_key(creator, name)?);
        }
        self.store.delete(&keys).await?;
        Ok(())
    }
}
