//! Whether a write is conditioned on the key's current state.

use crate::PreconditionToken;

/// A write-conditional precondition, in `oqueue-core`'s own vocabulary.
///
/// ⚠️ **Deliberately not S3 or GCS shaped.** Rendering this to S3's
/// `If-None-Match: *` / `If-Match: <etag>` or GCS's `ifGenerationMatch=0` /
/// `=N` is each backend's own job, in `oqueue-store` — a type named after
/// either backend does not belong in this crate (NFR-51). Only two states
/// exist; there is no third variant for "matches nothing in particular" or
/// any other combination neither backend can actually express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// Succeed only if nothing currently exists under the key.
    ///
    /// S3: `If-None-Match: *`. GCS: `ifGenerationMatch=0`.
    IfAbsent,
    /// Succeed only if the key's current state matches this token.
    ///
    /// S3: `If-Match: <etag>`. GCS: `ifGenerationMatch=<generation>`.
    IfMatches(PreconditionToken),
}
