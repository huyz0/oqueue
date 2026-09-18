//! Reading a partition manifest back, from bytes an object store returned.
//!
//! ⚠️ **Its own module because writing and reading are different risks**, the
//! split `bundle.rs`/`bundle_footer.rs` and `composite.rs`/`composite/manifest.rs`
//! both make: the writer's inputs come from this process and this parser's
//! come off the network.

use super::{
    ManifestEntry, PARTITION_MANIFEST_MAGIC, PARTITION_MANIFEST_TRAILER_LEN,
    PARTITION_MANIFEST_VERSION,
};
use crate::cursor::Cursor;
use crate::{ByteRange, Error, ObjectKey, Offset, Result};
use std::collections::HashSet;

/// A partition's history: its objects in offset order, and where the rest is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionManifest {
    entries: Vec<ManifestEntry>,
    previous: Option<ObjectKey>,
}

impl PartitionManifest {
    /// The objects, ascending by offset.
    #[must_use]
    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }

    /// The manifest this one spilled from, if it is not the first.
    #[must_use]
    pub const fn previous(&self) -> Option<&ObjectKey> {
        self.previous.as_ref()
    }

    /// The entry holding `offset`, if this manifest covers it.
    ///
    /// ⚠️ **A binary search, which is what the entries being contiguous and
    /// ordered buys.** `PartitionManifestBuilder::push` refuses a gap and an
    /// overlap, and the parser re-checks both, so a `None` here means the
    /// offset is older than this manifest — follow [`previous`](Self::previous)
    /// — or newer than it, in which case it is in the tail.
    #[must_use]
    pub fn find(&self, offset: Offset) -> Option<&ManifestEntry> {
        let at = self
            .entries
            .partition_point(|entry| entry.base_offset() <= offset);
        let entry = self.entries.get(at.checked_sub(1)?)?;
        // `end_offset` can only fail on an entry the fold produced wrongly,
        // and a failure there means "not in this manifest", which is the
        // safe answer: the caller follows the chain and finds nothing.
        match entry.end_offset() {
            Ok(end) if offset < end => Some(entry),
            _ => None,
        }
    }
}

/// Reads a partition manifest back out of its payload.
///
/// # Errors
///
/// [`Error::MalformedPartitionManifest`] if the bytes are too short, if the
/// declared length does not fit them exactly, if a field runs past the end, if
/// a key is not UTF-8, if an entry names an object another entry named, or if
/// the entries leave a gap or overlap.
/// [`Error::NotAPartitionManifest`] if the magic is absent, and
/// [`Error::UnknownPartitionManifestVersion`] if the version is not one this
/// build knows. [`Error::EmptyBundle`] if it declares no entry.
pub fn parse_partition_manifest(bytes: &[u8]) -> Result<PartitionManifest> {
    let total = bytes.len();
    let trailer_at = total
        .checked_sub(PARTITION_MANIFEST_TRAILER_LEN)
        .ok_or(Error::MalformedPartitionManifest { at: 0 })?;
    if bytes[total - PARTITION_MANIFEST_MAGIC.len()..] != PARTITION_MANIFEST_MAGIC {
        return Err(Error::NotAPartitionManifest);
    }
    let mut trailer = Cursor::new(&bytes[trailer_at..], |at| {
        Error::MalformedPartitionManifest { at }
    });
    let count = trailer.u32()?;
    let version = trailer.byte()?;
    if version != PARTITION_MANIFEST_VERSION {
        return Err(Error::UnknownPartitionManifestVersion { version });
    }
    let body_len = trailer.u32()? as usize;
    if count == 0 {
        return Err(Error::EmptyBundle);
    }
    // Exactly: a body shorter than the space before the trailer leaves bytes
    // nothing describes.
    if body_len != trailer_at {
        return Err(Error::MalformedPartitionManifest { at: trailer_at });
    }

    let mut cursor = Cursor::new(&bytes[..trailer_at], |at| {
        Error::MalformedPartitionManifest { at }
    });
    let previous = decode_optional_key(&mut cursor)?;
    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut seen: HashSet<ObjectKey> = HashSet::new();
    for _ in 0..count {
        let entry = decode_entry(&mut cursor)?;
        if !seen.insert(entry.object().clone()) {
            return Err(Error::MalformedPartitionManifest { at: cursor.at() });
        }
        // ⚠️ **Contiguity is re-checked here, not trusted from the builder**,
        // for the reason every read-side check in this repository exists: the
        // builder cannot reach bytes that are already in a bucket. A gap is
        // records nothing can serve and an overlap is records served twice,
        // and `find`'s binary search cannot tell either from a healthy
        // manifest.
        if let Some(last) = entries.last()
            && last.end_offset()? != entry.base_offset()
        {
            return Err(Error::MalformedPartitionManifest { at: cursor.at() });
        }
        entries.push(entry);
    }
    if cursor.at() != trailer_at {
        return Err(Error::MalformedPartitionManifest { at: cursor.at() });
    }
    Ok(PartitionManifest { entries, previous })
}

fn decode_optional_key(cursor: &mut Cursor<'_>) -> Result<Option<ObjectKey>> {
    let len = cursor.u16()? as usize;
    if len == 0 {
        return Ok(None);
    }
    Ok(Some(ObjectKey::new(cursor.string(len)?)?))
}

fn decode_entry(cursor: &mut Cursor<'_>) -> Result<ManifestEntry> {
    let at = cursor.at();
    let key_len = cursor.u16()? as usize;
    if key_len == 0 {
        return Err(Error::MalformedPartitionManifest { at });
    }
    let object = ObjectKey::new(cursor.string(key_len)?)?;
    let base_offset = Offset::new(cursor.i64()?)?;
    let record_count = cursor.u32()?;
    let offset = cursor.u64()?;
    let length = cursor.u64()?;
    ManifestEntry::new(
        object,
        base_offset,
        record_count,
        ByteRange::bounded(offset, length)?,
    )
}
