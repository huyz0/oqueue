//! Composite objects: N objects collapsed into one, with zero data movement.
//!
//! ⚠️ **The cheapest compaction there is, because it moves no records.** A
//! merge (`oqueue-compact`) reads every input and writes their bytes out
//! again; a composite writes a **manifest** — the component objects' keys and
//! a copy of their index entries — and leaves every record byte exactly where
//! it already was. What drops is the *object count* the coordinator carries,
//! which is doc 14's metadata cost and NFR-11's quota, not the cloud bill's
//! byte count.
//!
//! ⚠️ **Built from the start, not bolted on** (`M5.md` task 8). The reference
//! system added this at 90% of an object-count ceiling, which is the point at
//! which the mechanism is needed and the worst point at which to introduce it.
//!
//! ⚠️ **It buys object count and costs one indirection.** A fetch through a
//! composite resolves a `(topic, partition)` to a *component* key and a byte
//! range inside that component, so the read is still one ranged GET of one
//! object — but finding out which one means holding the manifest. That is the
//! trade `M5.9`'s re-keying makes worthwhile: the manifest is the coarse index
//! for the objects it names.
//!
//! ⚠️ **A composite is never a bundle, and the format says so in its own
//! trailer.** Both formats end in a count, a version and a length, so a
//! bundle's trailer would parse as a composite's and yield component keys
//! built out of record bytes — the failure that serves one topic's records as
//! another's. [`COMPOSITE_MAGIC`] is what stops it. ⚠️ **The bundle format
//! carries no such marker**, so the guard is one-directional: composite bytes
//! handed to `parse_footer` are refused by its own length checks rather than
//! by a name, and that asymmetry is worth knowing before relying on it.

mod manifest;

pub use manifest::{Component, parse_composite};

use crate::{ByteRange, Error, ObjectKey, PartitionId, Region, RegionAlg, Result, TopicId};
use std::collections::HashSet;

/// The manifest format's version.
///
/// ⚠️ Separate from `BUNDLE_FORMAT_VERSION` because they are separate formats
/// that happen to share a shape. A composite whose components are bundles is
/// the only case today, and tying the two versions together would make a
/// change to either a change to both.
pub const COMPOSITE_FORMAT_VERSION: u8 = 1;

/// What a composite's trailer ends with, so that bytes are self-identifying.
///
/// ⚠️ **Four bytes against the failure of reading a bundle as a composite.**
/// Both formats end in a count, a version and a length, so without a marker a
/// bundle's trailer parses as a composite's and yields component keys built
/// out of record bytes.
pub const COMPOSITE_MAGIC: [u8; 4] = *b"OQCM";

/// Trailer: `component_count` (u32), version (u8), `manifest_len` (u32), magic.
pub const COMPOSITE_TRAILER_LEN: usize = 4 + 1 + 4 + COMPOSITE_MAGIC.len();

/// Builds a composite's manifest.
///
/// ⚠️ **It writes no record bytes and cannot**: `push` takes a component's key
/// and the regions that component already holds, so the only bytes this
/// produces are the manifest's own. That is the property `M5.8`'s acceptance
/// asks a test to observe, and it is structural rather than tested into place.
#[derive(Debug, Default)]
pub struct CompositeBuilder {
    components: Vec<Component>,
    seen: HashSet<ObjectKey>,
}

impl CompositeBuilder {
    /// An empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
            seen: HashSet::new(),
        }
    }

    /// Adds one component object and the regions it holds.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyBundle`] if the component holds no region — an object
    /// with nothing in it is not a component, and admitting one would let a
    /// composite claim a count it cannot serve.
    /// [`Error::UnboundedRegion`] if a region's range is
    /// [`ByteRange::Full`]: a component's regions are offsets *inside* that
    /// component, and one claiming the whole of it would have a reader decode
    /// a neighbour's records as its own.
    /// [`Error::IndexObjectMismatch`] if the object is already a component.
    /// ⚠️ **Which is the composite's form of the hazard `merge`'s `tiling()`
    /// refuses by name**: a manifest naming one object twice resolves to two
    /// runs over the same bytes, so a fetch counts those records twice and a
    /// retention pass believes the object holds twice what it does. Nothing
    /// downstream can tell that from an object genuinely holding two slices.
    pub fn push(&mut self, object: ObjectKey, regions: Vec<Region>) -> Result<()> {
        if regions.is_empty() {
            return Err(Error::EmptyBundle);
        }
        if self.seen.contains(&object) {
            return Err(Error::IndexObjectMismatch);
        }
        for region in &regions {
            if matches!(region.bytes(), ByteRange::Full) {
                return Err(Error::UnboundedRegion);
            }
        }
        self.seen.insert(object.clone());
        self.components.push(Component::new(object, regions));
        Ok(())
    }

    /// How many components it holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.components.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Encodes the manifest.
    ///
    /// ⚠️ **The whole payload, not a footer appended to records.** A composite
    /// object *is* its manifest; there is no data half.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyBundle`] if it holds no component. [`Error::BundleTooLarge`]
    /// if a key, a topic name, the component count or the manifest's length
    /// exceeds what the format's fields can carry.
    pub fn seal(self) -> Result<Vec<u8>> {
        if self.components.is_empty() {
            return Err(Error::EmptyBundle);
        }
        let mut out = Vec::new();
        for component in &self.components {
            encode_component(component, &mut out)?;
        }
        let manifest_len = u32::try_from(out.len()).map_err(|_| Error::BundleTooLarge)?;
        let count = u32::try_from(self.components.len()).map_err(|_| Error::BundleTooLarge)?;
        out.extend_from_slice(&count.to_be_bytes());
        out.push(COMPOSITE_FORMAT_VERSION);
        out.extend_from_slice(&manifest_len.to_be_bytes());
        out.extend_from_slice(&COMPOSITE_MAGIC);
        Ok(out)
    }
}

fn encode_component(component: &Component, out: &mut Vec<u8>) -> Result<()> {
    let key = component.object().as_str().as_bytes();
    let key_len = u16::try_from(key.len()).map_err(|_| Error::BundleTooLarge)?;
    out.extend_from_slice(&key_len.to_be_bytes());
    out.extend_from_slice(key);
    let region_count =
        u32::try_from(component.regions().len()).map_err(|_| Error::BundleTooLarge)?;
    out.extend_from_slice(&region_count.to_be_bytes());
    for region in component.regions() {
        let topic = region.topic().as_str().as_bytes();
        let name_len = u16::try_from(topic.len()).map_err(|_| Error::BundleTooLarge)?;
        out.extend_from_slice(&name_len.to_be_bytes());
        out.extend_from_slice(topic);
        // `PartitionId` is never negative (its own invariant), so this is a
        // widening rather than a reinterpretation.
        out.extend_from_slice(&region.partition().get().unsigned_abs().to_be_bytes());
        out.extend_from_slice(&region.record_count().to_be_bytes());
        let ByteRange::Bounded(bounded) = region.bytes() else {
            // `push` refuses one, so this is unreachable through the builder --
            // and it is here because `Region` is public and this writes a
            // durable format: an unbounded range would produce a manifest
            // `parse_composite` refuses, after `seal` returned `Ok` and the
            // PUT landed.
            return Err(Error::UnboundedRegion);
        };
        out.extend_from_slice(&bounded.offset().to_be_bytes());
        out.extend_from_slice(&bounded.length().to_be_bytes());
        // ⚠️ **A composite manifest has no envelope field**, so a sealed
        // region cannot be described by it: writing the algorithm code without
        // the key id, wrapped DEK and nonce that undo it would make the data
        // durable and permanently unopenable. Unreachable until `M8.6` seals
        // anything, and a refusal from the day those bytes can exist rather
        // than from the day someone notices.
        if region.alg() != RegionAlg::None {
            return Err(Error::SealedRegionNotRepresentable {
                context: "a composite manifest",
            });
        }
        out.push(region.alg().code());
    }
    Ok(())
}

/// Where one partition's records live, once a composite is resolved.
///
/// ⚠️ **A component key and a range inside it**, which is what makes a fetch
/// through a composite still one ranged GET of one object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located<'a> {
    object: &'a ObjectKey,
    bytes: ByteRange,
    record_count: u32,
}

impl<'a> Located<'a> {
    /// The component object holding the records.
    #[must_use]
    pub const fn object(&self) -> &'a ObjectKey {
        self.object
    }

    /// Where inside that component they are.
    #[must_use]
    pub const fn bytes(&self) -> ByteRange {
        self.bytes
    }

    /// How many records the region holds.
    #[must_use]
    pub const fn record_count(&self) -> u32 {
        self.record_count
    }
}

/// Every place a `(topic, partition)`'s records live across the components, in
/// the order the manifest lists them.
///
/// ⚠️ **A `Vec`, because one partition may appear in several components** —
/// that is the ordinary case, since a composite collapses objects that each
/// held a slice of the same partition. Returning the first would serve one
/// slice and lose the rest.
#[must_use]
pub fn locate<'a>(
    components: &'a [Component],
    topic: &TopicId,
    partition: PartitionId,
) -> Vec<Located<'a>> {
    let mut found = Vec::new();
    for component in components {
        for region in component.regions() {
            if region.topic() == topic && region.partition() == partition {
                found.push(Located {
                    object: component.object(),
                    bytes: region.bytes(),
                    record_count: region.record_count(),
                });
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::{CompositeBuilder, encode_component};
    use crate::bundle::RegionAlg;
    use crate::composite::Component;
    use crate::{ByteRange, Error, ObjectKey, PartitionId, Region, TopicId};

    /// ⚠️ **Inside the crate because nothing outside it can build one.**
    /// `Region`'s fields are crate-private and no public constructor takes a
    /// `ByteRange`, so the unbounded case is unreachable from an integration
    /// test — and it is exactly the case that would have a reader decode a
    /// neighbour's records as its own, so it is tested where it is reachable
    /// rather than left to a comment.
    fn unbounded() -> Region {
        Region {
            topic: TopicId::new("t").expect("a valid topic"),
            partition: PartitionId::new(0).expect("a valid partition"),
            bytes: ByteRange::Full,
            record_count: 1,
            alg: RegionAlg::None,
            envelope: None,
        }
    }

    fn bounded() -> Region {
        Region {
            bytes: ByteRange::bounded(0, 32).expect("a valid range"),
            ..unbounded()
        }
    }

    fn key() -> ObjectKey {
        ObjectKey::new("a").expect("a valid key")
    }

    /// ⚠️ **`len` and `is_empty` are what `compose` reports as its component
    /// count**, so a builder that miscounted would have the executor report a
    /// collapse it did not make.
    #[test]
    fn a_builder_counts_what_it_holds() {
        let mut builder = CompositeBuilder::new();
        assert!(builder.is_empty());
        assert_eq!(builder.len(), 0);
        builder
            .push(key(), vec![bounded()])
            .expect("a valid component");
        assert!(!builder.is_empty());
        assert_eq!(builder.len(), 1);
        builder
            .push(ObjectKey::new("b").expect("a valid key"), vec![bounded()])
            .expect("a second component");
        assert_eq!(builder.len(), 2);
    }

    #[test]
    fn a_component_with_an_unbounded_region_is_refused() {
        let mut builder = CompositeBuilder::new();
        assert!(matches!(
            builder.push(key(), vec![unbounded()]),
            Err(Error::UnboundedRegion)
        ));
        assert!(builder.is_empty(), "and nothing was added");
    }

    /// The writer's own guard, which `push` makes unreachable and which is
    /// here because `Region` is public and this writes a durable format.
    #[test]
    fn encoding_an_unbounded_region_is_refused() {
        let mut out = Vec::new();
        let component = Component::new(key(), vec![unbounded()]);
        assert!(matches!(
            encode_component(&component, &mut out),
            Err(Error::UnboundedRegion)
        ));
    }
}
