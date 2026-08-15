//! Topic identity.

use crate::{Error, Result};

/// A topic name, guaranteed non-empty.
///
/// # Invariant
///
/// **The wrapped string is never empty.** There is no way to build one that is:
/// the only constructor rejects it, and the field is private.
///
/// ⚠️ **That is the whole invariant, deliberately.** Kafka constrains topic
/// names further — length, character set, the reserved `.` and `..` — and those
/// belong to `M2`, where the protocol is parsed and a client's expectations are
/// known. Encoding a guess here would be an invariant every later layer has to
/// work around.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TopicId(String);

impl TopicId {
    /// Builds a topic id.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyTopicId`] if `name` is empty.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::EmptyTopicId);
        }
        Ok(Self(name))
    }

    /// The topic name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for TopicId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}
