use std::mem::size_of;

use super::ObjectStoreTopicCatalog;
use crate::{Error, ObjectKey, Result, TopicId, TopicRetentionUpdate};

const RETENTION_FORMAT: u8 = 1;

impl ObjectStoreTopicCatalog {
    fn configs(&self) -> String {
        format!("{}/config/", self.prefix)
    }

    pub(super) fn retention_key(&self, name: &TopicId) -> Result<ObjectKey> {
        ObjectKey::new(format!(
            "{}{}",
            self.configs(),
            super::hex(name.as_str().as_bytes())
        ))
    }

    pub(super) async fn read_retention(&self, name: &TopicId) -> Result<Option<i64>> {
        if self.find(name).await?.is_none() {
            return Ok(None);
        }
        let Some(bytes) = self.read(&self.retention_key(name)?).await? else {
            return Ok(None);
        };
        if bytes.len() != 1 + size_of::<i64>() || bytes[0] != RETENTION_FORMAT {
            return Err(Error::MalformedMetadataSegment { at: 0 });
        }
        let raw: [u8; 8] = bytes[1..]
            .try_into()
            .map_err(|_| Error::MalformedMetadataSegment { at: 1 })?;
        Ok(Some(i64::from_be_bytes(raw)))
    }

    pub(super) async fn write_retention(
        &self,
        name: &TopicId,
        retention_ms: Option<i64>,
    ) -> Result<TopicRetentionUpdate> {
        if self.find(name).await?.is_none() {
            return Ok(TopicRetentionUpdate::Missing);
        }
        let key = self.retention_key(name)?;
        match retention_ms {
            Some(value) => {
                let mut body = Vec::with_capacity(1 + size_of::<i64>());
                body.push(RETENTION_FORMAT);
                body.extend_from_slice(&value.to_be_bytes());
                self.store.put(&key, body, None).await?;
            }
            None => self.store.delete(std::slice::from_ref(&key)).await?,
        }
        if self.find(name).await?.is_none() {
            return Ok(TopicRetentionUpdate::Missing);
        }
        Ok(TopicRetentionUpdate::Applied(retention_ms))
    }
}
