//! What a successful [`crate::ObjectStore::put`] hands back.

/// An opaque handle to one write of one object, for a later conditional
/// write to compare against.
///
/// ⚠️ **Opaque by design, on two axes.** It is **not comparable across
/// backends** — an S3 `ETag` and a GCS generation number mean nothing next to
/// each other, and nothing in this workspace ever holds tokens from two
/// backends for the same key. And it is **not a content hash**: a multipart
/// upload's `ETag` is not a hash of the assembled object, and SSE-KMS can
/// change an `ETag` without changing a single content byte, so anything that
/// tried to derive one from the payload would be wrong on both. Its only
/// contract is: pass the token a write returned back into a later conditional
/// write on the same key, on the same backend, and that backend decides
/// whether it still matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreconditionToken(String);

impl PreconditionToken {
    /// Wraps an opaque token a backend produced.
    #[must_use]
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token, exactly as the backend produced it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What is known about an object immediately after a successful write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    /// The size of the object that was written, in bytes.
    pub size: u64,
    /// A handle to this write, for a later conditional write to present.
    pub precondition_token: PreconditionToken,
}
