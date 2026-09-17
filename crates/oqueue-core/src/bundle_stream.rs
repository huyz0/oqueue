//! Assembling a bundled object without holding it.
//!
//! ⚠️ **The reason this exists at all.** [`BundleBuilder`] accumulates every
//! region's bytes in one `Vec` and hands the whole object to
//! [`ObjectStore::put`](crate::ObjectStore::put); that is right for a flush,
//! whose size is known when the write starts, and wrong for a compaction merge,
//! whose output is as large as its plan's budget allows. `ADR-0037` adds the
//! streaming half of the seam, and this is what turns it into an object: bytes
//! go out as parts as they fill, and the footer — which is at the object's tail
//! by design, so that a reader knows the format before it parses the regions —
//! is the last part.
//!
//! ⚠️ **What is bounded is the payload, not the footer.** Region metadata
//! accumulates until `finish`, because a footer describes every region and
//! cannot be written incrementally. That is ~32 bytes plus a topic name per
//! region against the region's own bytes, so an object of N 8 MiB parts holds
//! kilobytes of metadata — but it is not zero, and a caller compacting a
//! partition into millions of tiny regions would find it.

use crate::bundle::{BundleBuilder, PushedRecords};
use crate::{CommittedSpan, MultipartWriter, ObjectKey, ObjectStore, PartitionId, Result, TopicId};

/// How many bytes one part carries.
///
/// ⚠️ **Above S3's 5 MiB minimum**, which applies to every part but the last: a
/// smaller part is refused by the backend rather than by this crate, which is
/// the wrong place to find out. 8 MiB is the smallest power of two clear of it,
/// and UNDERIVED beyond that — `M14` measures whether a larger part buys
/// anything.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const BUNDLE_PART_BYTES: usize = 8_388_608;

/// A bundled object being written to a streaming writer.
///
/// ⚠️ **Region metadata is built through [`BundleBuilder`] rather than beside
/// it**, so the footer this writes is the one `parse_footer` reads and there is
/// one encoder rather than two. What is not shared is where the bytes go.
#[derive(Debug)]
pub struct BundleStream<'store> {
    writer: Box<dyn MultipartWriter<'store> + 'store>,
    /// Region metadata only: `push` hands `BundleBuilder` a zero-length slice
    /// and records the real range itself.
    regions: BundleBuilder,
    buffer: Vec<u8>,
    written: u64,
}

impl<'store> BundleStream<'store> {
    /// Opens a streaming write of a bundled object.
    ///
    /// # Errors
    ///
    /// Whatever the store says about opening a multipart upload.
    pub async fn open<S>(store: &'store S, key: &'store ObjectKey) -> Result<Self>
    where
        S: ObjectStore + ?Sized,
    {
        Ok(Self {
            writer: store.open_multipart(key).await?,
            regions: BundleBuilder::new(),
            buffer: Vec::with_capacity(BUNDLE_PART_BYTES),
            written: 0,
        })
    }

    /// Appends one `(topic, partition)`'s records, flushing full parts.
    ///
    /// # Errors
    ///
    /// [`BundleBuilder::push`]'s own errors, and whatever the writer says.
    pub async fn push(
        &mut self,
        topic: TopicId,
        partition: PartitionId,
        pushed: PushedRecords,
        records: &[u8],
    ) -> Result<()> {
        self.regions
            .push_streamed(topic, partition, pushed, (self.written, records.len()))?;
        self.buffer.extend_from_slice(records);
        self.written = self
            .written
            .saturating_add(records.len().try_into().unwrap_or(u64::MAX));
        while self.buffer.len() >= BUNDLE_PART_BYTES {
            let rest = self.buffer.split_off(BUNDLE_PART_BYTES);
            let part = core::mem::replace(&mut self.buffer, rest);
            self.writer.write_part(part).await?;
        }
        Ok(())
    }

    /// How many bytes are buffered but not yet sent.
    ///
    /// ⚠️ **The memory bound, made observable.** A streaming writer's whole
    /// claim is that the process does not hold the object, and a test that
    /// cannot see this number can only assert the claim by believing it.
    #[must_use]
    pub const fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Writes the footer and seals the object.
    ///
    /// # Errors
    ///
    /// [`BundleBuilder::seal`]'s own errors, and whatever the writer says.
    pub async fn finish(mut self) -> Result<Vec<CommittedSpan>> {
        let (footer, spans) = self.regions.into_footer()?;
        self.buffer.extend_from_slice(&footer);
        // ⚠️ **Whatever is left goes as one part, however small.** Every
        // backend bounds a part's size from below *except the last*, which is
        // exactly this one.
        self.writer.write_part(self.buffer).await?;
        self.writer.finish().await?;
        Ok(spans)
    }
}
