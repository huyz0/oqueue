//! The listing seam `ADR-0009` §2 named and `M7.3a` builds.

use crate::{BoxFuture, ObjectKey, Result};

/// Enumerates keys in object storage, in pages.
///
/// ⚠️ **A separate seam from [`ObjectStore`](crate::ObjectStore), so NFR-30
/// stays structural.** `ObjectStore` carries no `list` (`ADR-0009` §2), and
/// only a component explicitly handed a `MaintenanceStore` can LIST. The
/// metadata log and the read path are never handed one: `ADR-0046`'s rule
/// that the metadata log opens without a LIST still holds. The first holder is
/// the topic catalog (`ADR-0049`), which must page topic names in order.
pub trait MaintenanceStore: Send + Sync + core::fmt::Debug {
    /// Keys under `prefix`, strictly after `after` when given, in byte order,
    /// at most `limit`.
    ///
    /// ⚠️ **Plain string-prefix semantics, as S3 gives them.** `prefix` is not a
    /// path segment: `"topics/a"` matches `"topics/a/x"` *and* `"topics/ab"`. A
    /// caller that wants one directory names the trailing `/`. The empty prefix
    /// matches every key.
    ///
    /// ⚠️ **Strictly after, and ordered by bytes.** `after` itself is never
    /// returned, whether or not it exists, so a caller pages by passing the last
    /// key of the previous page. Byte order on the UTF-8 key is what S3's
    /// `ListObjectsV2` and GCS's list both answer in; no locale order is implied.
    ///
    /// `limit` 0 answers an empty page. A page shorter than `limit` means there
    /// was nothing more *at the time of the listing*.
    ///
    /// ⚠️ **No snapshot.** A key written or deleted while a caller pages may or
    /// may not appear, exactly as S3 promises no more. What is promised is that
    /// every key returned was present when its page was read, and that pages
    /// never go backwards.
    ///
    /// # Errors
    ///
    /// Implementation-defined; the fake does not fail *by default* — same
    /// storm-driven exception as [`ObjectStore::put`](crate::ObjectStore::put)'s,
    /// with a [`FaultConfig`](crate::FaultConfig) installed.
    fn list<'a>(
        &'a self,
        prefix: &'a str,
        after: Option<&'a ObjectKey>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<ObjectKey>>>;
}
