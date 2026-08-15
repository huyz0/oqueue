//! Object-storage keys.

use crate::{Error, Result};

/// A key naming one object in the store, guaranteed non-empty.
///
/// # Invariant
///
/// **The wrapped string is never empty.**
///
/// ⚠️ **No length or character limit is imposed here, deliberately.** Those are
/// the backend's, they differ between S3 and GCS, and `oqueue-core` names no
/// backend (NFR-51). `M1` is where a store implementation validates against the
/// limits of the service it actually talks to — a limit guessed here would be
/// either wrong for one backend or wrong for both.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    /// Builds an object key.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyObjectKey`] if `key` is empty.
    pub fn new(key: impl Into<String>) -> Result<Self> {
        let key = key.into();
        if key.is_empty() {
            return Err(Error::EmptyObjectKey);
        }
        Ok(Self(key))
    }

    /// The key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl core::fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}
