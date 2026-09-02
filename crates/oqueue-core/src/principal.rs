//! An authenticated principal's identity.

use crate::{Error, Result};

/// The identity every request in this broker is scoped to (FR-4, FR-40),
/// guaranteed non-empty.
///
/// # Invariant
///
/// **The wrapped string is never empty.** [`TopicId`](crate::TopicId)'s own
/// precedent: the only constructor rejects it, and the field is private.
///
/// ⚠️ **This is the authenticated name, never the credential.** `M9.4`'s
/// `SASL/PLAIN` mechanism carries a password alongside this name during
/// authentication itself, but a `Principal` is what survives that exchange
/// — every later request (`M9.7`'s authorization decision point, `M9.8`'s
/// forward index) is scoped by this alone. A principal's name identifies,
/// the way a topic name does; it is not `FR-44`'s "secret or key material"
/// and carries no [`Redacted`](crate::Redacted) wrapper, deliberately —
/// wrapping a value that is not sensitive would hide from a reader which
/// fields in this crate actually need the caution `Redacted` signals.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Principal(String);

impl Principal {
    /// Builds a principal.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyPrincipal`] if `name` is empty.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() {
            return Err(Error::EmptyPrincipal);
        }
        Ok(Self(name))
    }

    /// The principal's name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for Principal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}
