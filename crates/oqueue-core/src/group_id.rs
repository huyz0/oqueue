//! Consumer group identity (`M4.1`, FR-22).

use crate::{Error, Result};

/// A consumer group name, guaranteed non-empty.
///
/// # Invariant
///
/// **The wrapped string is never empty.** [`crate::TopicId`]'s own precedent:
/// the only constructor rejects it, and the field is private.
///
/// ⚠️ Kafka constrains group names further — length, character set — and
/// those belong to the wire layer that parses a client's request and knows
/// its own version-gated limits, the same split `TopicId`'s own doc already
/// draws for topic names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GroupId(String);

impl GroupId {
    /// Builds a group id.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyGroupId`] if `name` is empty.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::EmptyGroupId);
        }
        Ok(Self(name))
    }

    /// The group name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for GroupId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::GroupId;
    use crate::Error;

    #[test]
    fn rejects_empty() {
        assert_eq!(GroupId::new(""), Err(Error::EmptyGroupId));
    }

    #[test]
    fn keeps_a_non_empty_name() {
        assert_eq!(GroupId::new("orders").expect("valid").as_str(), "orders");
    }

    #[test]
    fn displays_as_the_bare_name() {
        assert_eq!(GroupId::new("orders").expect("valid").to_string(), "orders");
    }
}
