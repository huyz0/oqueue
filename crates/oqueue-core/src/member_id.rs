//! Consumer group member identity (`M4.1`, FR-20).

use crate::{Error, Result};

/// One member's identity within a group, guaranteed non-empty.
///
/// ⚠️ **Minted by the broker, never chosen by a client.** A `JoinGroup`
/// request's own `member_id` field is empty on a client's first join and
/// echoes a previously-minted value on every rejoin — this type is the
/// *minted* shape, not what arrives on the wire; `M4.7`'s own handler is
/// where minting happens (a fresh join) or where an echoed value is looked
/// up and validated (a rejoin), never invented for it here. The wire
/// decoder's own `Option<&str>`/empty-string handling is what tells the two
/// cases apart before either ever reaches this type.
///
/// # Invariant
///
/// **The wrapped string is never empty.** [`crate::GroupId`]'s own precedent:
/// the only constructor rejects it, and the field is private.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemberId(String);

impl MemberId {
    /// Wraps an already-minted member id.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyMemberId`] if `id` is empty — the broker's own minting
    /// scheme (`M4.7`) never produces one, so this is a defensive check
    /// against a malformed rejoin, not a case this constructor invents.
    pub fn new(id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(Error::EmptyMemberId);
        }
        Ok(Self(id))
    }

    /// The member id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for MemberId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::MemberId;
    use crate::Error;

    #[test]
    fn rejects_empty() {
        assert_eq!(MemberId::new(""), Err(Error::EmptyMemberId));
    }

    #[test]
    fn keeps_a_non_empty_id() {
        assert_eq!(
            MemberId::new("consumer-1-abc").expect("valid").as_str(),
            "consumer-1-abc"
        );
    }

    #[test]
    fn displays_as_the_bare_id() {
        assert_eq!(
            MemberId::new("consumer-1-abc").expect("valid").to_string(),
            "consumer-1-abc"
        );
    }
}
