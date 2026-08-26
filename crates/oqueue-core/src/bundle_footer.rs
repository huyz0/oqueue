//! Reading a bundled object's footer back, from bytes an object store returned.
//!
//! ⚠️ **Its own module because writing and reading are different risks**, and
//! because `bundle.rs` reached the 500-line limit. The writer's inputs come
//! from this process; this parser's come off the network, so `security.md`
//! rule 3 (nothing reachable from stored bytes may panic) and rule 4 (no
//! unchecked arithmetic) are the rules that govern it, and every length here
//! goes through one bounds-checked cursor rather than one check per field.

use crate::bundle::{BUNDLE_FORMAT_VERSION, Region, RegionAlg, TRAILER_LEN};
use crate::{ByteRange, Error, PartitionId, Result, TopicId};

/// Reads a bundle's footer back out of the object's tail.
///
/// `tail` must be the end of the object — everything from the footer's first
/// byte onward is enough, and so is the whole object. A reader that knows the
/// object's size GETs the last few kilobytes, and this finds the footer inside
/// them.
///
/// ⚠️ **`object_size` is required, and a tail is why.** The regions describe
/// offsets in the *object*, and a suffix cannot say whether one reaches past
/// the payload — so the check that would catch a region claiming the whole
/// object needs the size the caller already has from its
/// [`ObjectMeta`](crate::ObjectMeta). Passing the wrong size weakens that one
/// check and nothing else.
///
/// ⚠️ **Every length is checked before it is used**, and that is not
/// defensiveness: this parses bytes an object store returned, and `M5` will
/// rewrite objects `M3` wrote, so a truncated or torn tail is a thing that
/// happens rather than a thing that would mean a bug. `security.md` rule 3 —
/// nothing reachable from stored bytes may panic.
///
/// # Errors
///
/// [`Error::MalformedBundleFooter`] if the tail is too short, the declared
/// footer length does not fit, or a region's fields run past the end.
/// [`Error::UnknownRegionAlg`] if a region names an algorithm this build does
/// not know. [`Error::EmptyBundle`] if the footer declares no regions.
pub fn parse_footer(tail: &[u8], object_size: u64) -> Result<Vec<Region>> {
    let bytes = tail;
    let trailer_at = bytes
        .len()
        .checked_sub(TRAILER_LEN)
        .ok_or(Error::MalformedBundleFooter { at: 0 })?;
    let trailer = &bytes[trailer_at..];
    let count = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]) as usize;
    let version = trailer[4];
    if version != BUNDLE_FORMAT_VERSION {
        return Err(Error::UnknownBundleFormat { version });
    }
    if count == 0 {
        return Err(Error::EmptyBundle);
    }
    let footer_len = u32::from_be_bytes([trailer[5], trailer[6], trailer[7], trailer[8]]) as usize;
    let footer_at = trailer_at
        .checked_sub(footer_len)
        .ok_or(Error::MalformedBundleFooter { at: trailer_at })?;

    let mut cursor = Cursor {
        bytes: &bytes[footer_at..trailer_at],
        at: 0,
    };
    // ⚠️ Not `with_capacity(count)`: `count` came off the wire, so reserving
    // from it lets a nine-byte tail ask for gigabytes. It grows as regions are
    // parsed, and the footer's own length is what bounds how many there can be.
    let mut regions = Vec::new();
    for _ in 0..count {
        regions.push(cursor.region()?);
    }
    if cursor.at != cursor.bytes.len() {
        return Err(Error::MalformedBundleFooter { at: cursor.at });
    }
    // ⚠️ Where the footer begins **in the object**, which is what the regions
    // are measured against — `footer_at` above is an index into whatever tail
    // the caller passed, and would let a short tail reject a valid object.
    let payload_end = object_size
        .checked_sub((footer_len + TRAILER_LEN) as u64)
        .ok_or(Error::MalformedBundleFooter { at: 0 })?;
    check_regions(&regions, payload_end)?;
    Ok(regions)
}

/// Re-establishes on the read path what [`BundleBuilder`] guarantees on the
/// write path.
///
/// ⚠️ **The invariant is a property of the *format*, not of the builder**, and
/// leaving it to the builder is how it stops being true: a corrupt length, a
/// torn write, or a bug in whatever rewrites these objects later (`M5`) yields
/// bytes no builder produced. Without this, a footer claiming
/// `offset = 0, length = u64::MAX` parses, and a reader honouring it GETs the
/// whole bundle and serves one topic's consumer another topic's records.
///
/// Four things, all cheap: regions are in ascending order, they do not
/// overlap, none reaches into the footer, and none claims records it has no
/// offsets for.
///
/// ⚠️ **The record-count check is here for the same reason it is in the
/// builder**, not for symmetry. [`BundleBuilder::push`] refuses a region with
/// no records because such a region is durable, indexed, billed and
/// unreachable, with no error raised anywhere — a fold that turns counts into
/// offsets advances none for it. A footer that arrives claiming
/// `record_count = 0` over four kilobytes has exactly that effect, and the
/// write-side refusal cannot reach it, because these bytes were not written by
/// this process.
///
/// ⚠️ The other half of "empty" — a zero-*length* range — needs nothing here:
/// [`ByteRange::bounded`] refuses it, so `region()` above fails with
/// [`Error::EmptyByteRange`] before a `Region` exists to check.
///
/// [`BundleBuilder::push`]: crate::BundleBuilder::push
/// [`ByteRange::bounded`]: crate::ByteRange::bounded
fn check_regions(regions: &[Region], payload_end: u64) -> Result<()> {
    let mut next_free = 0_u64;
    for (index, region) in regions.iter().enumerate() {
        let ByteRange::Bounded(bounds) = region.bytes else {
            return Err(Error::MalformedBundleFooter { at: index });
        };
        if region.record_count == 0 {
            return Err(Error::MalformedBundleFooter { at: index });
        }
        let end = bounds
            .offset()
            .checked_add(bounds.length())
            .ok_or(Error::MalformedBundleFooter { at: index })?;
        if bounds.offset() < next_free || end > payload_end {
            return Err(Error::MalformedBundleFooter { at: index });
        }
        next_free = end;
    }
    Ok(())
}

/// A bounds-checked walk over the footer's bytes.
///
/// ⚠️ Every read goes through [`take`](Cursor::take), so there is one place a
/// length is checked rather than one per field — which is the difference
/// between a parser that is right and one that is right so far.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(Error::MalformedBundleFooter { at: self.at })?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let bytes = self.take(8)?;
        let mut value = [0_u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(value))
    }

    fn region(&mut self) -> Result<Region> {
        let name_len = self.u16()? as usize;
        let at = self.at;
        let name = self.take(name_len)?;
        let topic = TopicId::new(
            core::str::from_utf8(name)
                .map_err(|_| Error::MalformedBundleFooter { at })?
                .to_owned(),
        )?;
        // ⚠️ `try_from`, not `as`: a partition id is an `i32` on the wire and
        // a `u32` in the footer, so a value above `i32::MAX` would wrap to a
        // negative one that `PartitionId::new` would then reject for the wrong
        // reason — or, worse, to a valid-looking different partition.
        let partition = PartitionId::new(
            i32::try_from(self.u32()?).map_err(|_| Error::MalformedBundleFooter { at: self.at })?,
        )?;
        let record_count = self.u32()?;
        let offset = self.u64()?;
        let length = self.u64()?;
        let bytes = ByteRange::bounded(offset, length)?;
        let alg = RegionAlg::from_code(self.take(1)?[0])?;
        Ok(Region {
            topic,
            partition,
            bytes,
            record_count,
            alg,
        })
    }
}
