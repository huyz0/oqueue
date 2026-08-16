//! Multipart/resumable upload bounds, backend-agnostic.
//!
//! ⚠️ **Not S3's or GCS's specific numbers.** [`MultipartLimits`]'s fields are
//! configurable, not hardcoded, because the two real backends do not share a
//! shape: S3 multipart upload bounds a part's size and a **count** of parts;
//! GCS resumable upload bounds a chunk to a multiple of 256 KiB with no
//! part-count analogue at all (doc 04 §5). A named `S3_LIMITS` constant here
//! would put S3-specific knowledge in the one crate NFR-51 forbids that in —
//! each backend constructs its own [`MultipartLimits`] when it needs one
//! (`M1.16`, `M1.17`).

use crate::{Error, Result};

/// The size and count bounds one multipart/resumable upload must satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultipartLimits {
    /// The minimum size of every part **except the last** — a trailing part
    /// may be smaller (S3 permits a last part of a few bytes; doc 04 §5).
    pub min_part_size: u64,
    /// The maximum size any single part may have.
    pub max_part_size: u64,
    /// The maximum number of parts one upload may have.
    pub max_parts: u32,
    /// The maximum total size of the assembled object.
    pub max_object_size: u64,
}

/// One part's byte length, checked against [`MultipartLimits`] at the point
/// it was added — a part number is a position, not a value, so it is not
/// wrapped as a type here; see [`MultipartSession::add_part`]'s return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PartSize(u64);

/// Accumulates parts for one multipart/resumable upload, rejecting a bound
/// violation at the point it becomes knowable rather than after the fact.
///
/// ⚠️ **Three of four bounds are checked as each part arrives; the fourth —
/// the minimum part size — is checked only at [`MultipartSession::finish`].**
/// A part below the minimum is legal *if it turns out to be the last one*,
/// and nothing knows a part is last until the caller stops adding them.
/// Checking the minimum eagerly would reject a legal upload whose final part
/// happens to be small — exactly the "streaming writer of unknown final
/// size" shape doc 04 §5 names as multipart's whole reason for existing.
#[derive(Debug, Clone)]
pub struct MultipartSession {
    limits: MultipartLimits,
    parts: Vec<PartSize>,
}

impl MultipartSession {
    /// A session with no parts yet, checked against `limits`.
    #[must_use]
    pub const fn new(limits: MultipartLimits) -> Self {
        Self {
            limits,
            parts: Vec::new(),
        }
    }

    /// Adds a part, returning its 1-based part number.
    ///
    /// # Errors
    ///
    /// [`Error::PartTooLarge`] if `bytes` exceeds [`MultipartLimits::max_part_size`].
    /// [`Error::TooManyParts`] if this would exceed [`MultipartLimits::max_parts`].
    /// [`Error::ObjectTooLarge`] if this would exceed [`MultipartLimits::max_object_size`].
    pub fn add_part(&mut self, bytes: u64) -> Result<u32> {
        if bytes > self.limits.max_part_size {
            return Err(Error::PartTooLarge {
                bytes,
                max: self.limits.max_part_size,
            });
        }
        let part_number = u32::try_from(self.parts.len())
            .ok()
            .and_then(|n| n.checked_add(1))
            .filter(|&n| n <= self.limits.max_parts)
            .ok_or(Error::TooManyParts {
                max: self.limits.max_parts,
            })?;
        let fits = self
            .parts
            .iter()
            .map(|p| p.0)
            .try_fold(bytes, u64::checked_add)
            .is_some_and(|total| total <= self.limits.max_object_size);
        if !fits {
            return Err(Error::ObjectTooLarge {
                max: self.limits.max_object_size,
            });
        }
        self.parts.push(PartSize(bytes));
        Ok(part_number)
    }

    /// Finalizes the session: every part but the last must meet
    /// [`MultipartLimits::min_part_size`].
    ///
    /// # Errors
    ///
    /// [`Error::PartTooSmall`] naming the first non-last part under the
    /// minimum. An empty session (no parts at all) has none to violate and
    /// finishes with an empty list.
    pub fn finish(self) -> Result<Vec<u64>> {
        let last_index = self.parts.len().checked_sub(1);
        for (index, part) in self.parts.iter().enumerate() {
            let is_last = Some(index) == last_index;
            if !is_last && part.0 < self.limits.min_part_size {
                return Err(Error::PartTooSmall {
                    bytes: part.0,
                    min: self.limits.min_part_size,
                });
            }
        }
        Ok(self.parts.into_iter().map(|p| p.0).collect())
    }
}
