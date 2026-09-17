// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module, and this whole module is one. `pub(crate)` is the
// visibility that is true — `s3.rs` constructs the writer and nothing outside
// this crate may — so the lint that disagrees is the one allowed, as
// `oqueue-core`'s `bundle.rs` does for the same reason.
#![allow(clippy::redundant_pub_crate)]

//! S3's streaming writer, over `object_store`'s own multipart upload.
//!
//! ⚠️ **Its own module because `s3.rs` reached the 500-line limit** when `M5.6`
//! added it (`code-structure.md` rule 18), and because the whole-payload path
//! and the streaming one are two subjects: one knows its size and can carry a
//! precondition, the other knows neither.

use crate::classify::classify;
use crate::s3::put::S3_MULTIPART_LIMITS;
use oqueue_core::{
    BoxFuture, Error, MultipartSession, ObjectKey, ObjectMeta, PreconditionToken, Result,
};

/// A streaming write against S3, over `object_store`'s own multipart
/// upload.
///
/// ⚠️ **Unconditional at the seal** (`ADR-0037`): `object_store`'s public API
/// carries no `PutMode` to `CompleteMultipartUpload`, and the caller's answer
/// is a key nothing else will name rather than a precondition here.
impl<'store> StreamingWriter<'store> {
    /// Wraps an opened upload.
    pub(crate) fn new(
        key: &'store ObjectKey,
        upload: Box<dyn object_store::MultipartUpload>,
    ) -> Self {
        Self {
            key,
            upload,
            session: MultipartSession::new(S3_MULTIPART_LIMITS),
            bytes: 0,
            poisoned: false,
        }
    }
}

pub(crate) struct StreamingWriter<'store> {
    key: &'store ObjectKey,
    upload: Box<dyn object_store::MultipartUpload>,
    /// ⚠️ **The same validator the whole-payload path uses.** A part over the
    /// backend's maximum, or one part too many, is refused here rather than at
    /// `complete()` — after every part has been uploaded and paid for.
    session: MultipartSession,
    bytes: u64,
    /// ⚠️ **A failed part poisons the writer**, because the upload it belonged
    /// to has already been aborted: a retried part would be sent against an
    /// upload id the backend has forgotten, and the error it came back with
    /// would describe that rather than the original failure.
    poisoned: bool,
}

/// ⚠️ Prints nothing the vendor client holds, for `S3Store`'s own reason.
impl core::fmt::Debug for StreamingWriter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StreamingWriter")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl<'store> oqueue_core::MultipartWriter<'store> for StreamingWriter<'store> {
    fn write_part(&mut self, part: Vec<u8>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.poisoned {
                return Err(Error::Permanent);
            }
            let len = u64::try_from(part.len()).unwrap_or(u64::MAX);
            self.session.add_part(len)?;
            match self.upload.put_part(part.into()).await {
                Ok(()) => {
                    self.bytes = self.bytes.saturating_add(len);
                    Ok(())
                }
                Err(source) => {
                    // ⚠️ Best-effort abort, the same trade the whole-payload
                    // path records: a second failure here is a bucket
                    // lifecycle rule's problem, not this return path's.
                    let _ = self.upload.abort().await;
                    self.poisoned = true;
                    Err(classify(&source, self.key))
                }
            }
        })
    }

    fn finish(mut self: Box<Self>) -> BoxFuture<'store, Result<ObjectMeta>> {
        Box::pin(async move {
            if self.poisoned {
                return Err(Error::Permanent);
            }
            if self.bytes == 0 {
                let _ = self.upload.abort().await;
                return Err(Error::EmptyBundle);
            }
            match self.upload.complete().await {
                Ok(result) => {
                    let e_tag = result.e_tag.ok_or(Error::Permanent)?;
                    Ok(ObjectMeta {
                        size: self.bytes,
                        precondition_token: PreconditionToken::new(e_tag),
                    })
                }
                Err(source) => {
                    let _ = self.upload.abort().await;
                    Err(classify(&source, self.key))
                }
            }
        })
    }
}
