//! Reading a composite's manifest back, from bytes an object store returned.
//!
//! ⚠️ **Its own module because writing and reading are different risks**, the
//! same split `bundle.rs` and `bundle_footer.rs` make: the writer's inputs come
//! from this process, and this parser's come off the network. `security.md`
//! rule 3 (nothing reachable from stored bytes may panic) and rule 4 (no
//! unchecked arithmetic) are what govern it.

use super::{COMPOSITE_FORMAT_VERSION, COMPOSITE_MAGIC, COMPOSITE_TRAILER_LEN};
use crate::bundle::RegionAlg;
use crate::cursor::Cursor;
use crate::{ByteRange, Error, ObjectKey, PartitionId, Region, Result, TopicId};
use std::collections::HashSet;

/// One object a composite names, and the regions it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    object: ObjectKey,
    regions: Vec<Region>,
}

impl Component {
    /// Pairs an object with its regions.
    pub(super) const fn new(object: ObjectKey, regions: Vec<Region>) -> Self {
        Self { object, regions }
    }

    /// The object.
    #[must_use]
    pub const fn object(&self) -> &ObjectKey {
        &self.object
    }

    /// What it holds.
    #[must_use]
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }
}

/// Reads a composite's components back out of its payload.
///
/// ⚠️ **The whole object, not a tail.** A composite *is* its manifest, so
/// there is no payload to skip and no `object_size` to reconcile — the trailer
/// is the last [`COMPOSITE_TRAILER_LEN`] bytes of what is passed in, and the
/// declared length must reach exactly back to the start.
///
/// # Errors
///
/// [`Error::MalformedCompositeManifest`] if the bytes are too short to hold
/// the trailer, if the declared length does not fit them exactly, if a field
/// runs past the end, or if a key or topic name is not UTF-8.
/// [`Error::UnknownCompositeVersion`] if the trailer names a version this
/// build does not know, and [`Error::NotACompositeManifest`] if the magic is
/// absent — ⚠️ **which is what a bundle's bytes produce**, and is the whole
/// reason the magic is there.
/// [`Error::EmptyBundle`] if it declares no component, and whatever
/// [`ObjectKey::new`] or [`TopicId::new`] say about a name the format carried
/// but this build will not accept.
pub fn parse_composite(bytes: &[u8]) -> Result<Vec<Component>> {
    let total = bytes.len();
    let trailer_at = total
        .checked_sub(COMPOSITE_TRAILER_LEN)
        .ok_or(Error::MalformedCompositeManifest { at: 0 })?;
    let magic_at = total - COMPOSITE_MAGIC.len();
    if bytes[magic_at..] != COMPOSITE_MAGIC {
        return Err(Error::NotACompositeManifest);
    }
    let mut trailer = Cursor::new(&bytes[trailer_at..], |at| {
        Error::MalformedCompositeManifest { at }
    });
    let count = trailer.u32()?;
    let version = trailer.byte()?;
    if version != COMPOSITE_FORMAT_VERSION {
        return Err(Error::UnknownCompositeVersion { version });
    }
    let manifest_len = trailer.u32()? as usize;
    if count == 0 {
        return Err(Error::EmptyBundle);
    }
    // ⚠️ **Exactly**, not "at most": a manifest shorter than the space before
    // the trailer leaves bytes nothing describes, which is either a torn write
    // or a reader looking at the wrong object.
    if manifest_len != trailer_at {
        return Err(Error::MalformedCompositeManifest { at: trailer_at });
    }

    let mut cursor = Cursor::new(&bytes[..trailer_at], |at| {
        Error::MalformedCompositeManifest { at }
    });
    let mut components = Vec::new();
    // ⚠️ **The read side refuses a repeated object too**, and for the reason
    // the zero-record check below gives rather than as symmetry for its own
    // sake: `CompositeBuilder::push` cannot reach bytes that are already in a
    // bucket. A manifest naming one object twice resolves to two runs over the
    // same bytes, so a fetch counts those records twice and a retention pass
    // believes the object holds twice what it does.
    let mut seen: HashSet<ObjectKey> = HashSet::new();
    for _ in 0..count {
        let component = decode_component(&mut cursor)?;
        if !seen.insert(component.object().clone()) {
            return Err(Error::MalformedCompositeManifest { at: cursor.at() });
        }
        components.push(component);
    }
    // Every byte before the trailer belongs to a component, for the reason the
    // length check above gives.
    if cursor.at() != trailer_at {
        return Err(Error::MalformedCompositeManifest { at: cursor.at() });
    }
    Ok(components)
}

fn decode_component(cursor: &mut Cursor<'_>) -> Result<Component> {
    let key_len = cursor.u16()? as usize;
    let key = ObjectKey::new(cursor.string(key_len)?)?;
    let region_count = cursor.u32()?;
    if region_count == 0 {
        return Err(Error::EmptyBundle);
    }
    let mut regions = Vec::new();
    for _ in 0..region_count {
        regions.push(decode_region(cursor)?);
    }
    Ok(Component::new(key, regions))
}

fn decode_region(cursor: &mut Cursor<'_>) -> Result<Region> {
    let name_len = cursor.u16()? as usize;
    let topic = TopicId::new(cursor.string(name_len)?)?;
    let raw_partition = cursor.u32()?;
    let at = cursor.at();
    let partition = PartitionId::new(
        i32::try_from(raw_partition).map_err(|_| Error::MalformedCompositeManifest { at })?,
    )?;
    let at_count = cursor.at();
    let record_count = cursor.u32()?;
    // ⚠️ **A region holding no record is refused here, not only on the write
    // side** (`bundle_footer.rs` makes the same call for the same reason): a
    // zero-record region is durable, indexed, billed and unreachable, and the
    // builder's refusal cannot reach bytes that are already in a bucket. A
    // manifest is a *copy* of index entries read off the network, so the
    // argument transfers verbatim.
    if record_count == 0 {
        return Err(Error::MalformedCompositeManifest { at: at_count });
    }
    let offset = cursor.u64()?;
    let length = cursor.u64()?;
    let alg = RegionAlg::from_code(cursor.byte()?)?;
    Ok(Region {
        topic,
        partition,
        bytes: ByteRange::bounded(offset, length)?,
        record_count,
        alg,
    })
}
