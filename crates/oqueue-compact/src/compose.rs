//! Writing a composite: the compaction that moves no records.
//!
//! ⚠️ **One PUT of a manifest, and that is the whole operation.** `merge`
//! reads every input and writes their bytes out again; this reads every
//! input's *footer* and writes a manifest naming where those bytes already
//! are. What drops is the object count the coordinator carries — doc 14's
//! metadata cost and NFR-11's quota — and what does not move is a single
//! record byte.
//!
//! ⚠️ **So it is the cheap arm of a choice, not a replacement for `merge`.**
//! A composite leaves read amplification exactly where it was: a fetch still
//! touches whichever component holds the range. `M5.2`'s planner is what picks
//! between them, and the input that says which is what the sweep measured —
//! amplification is a merge's problem, object count is a composite's.

use oqueue_core::{
    Component, CompositeBuilder, ObjectKey, ObjectRef, ObjectStore, Result, parse_footer,
};

/// What composing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeOutcome {
    gets: usize,
    components: usize,
    manifest_bytes: usize,
}

impl ComposeOutcome {
    /// Footer reads it issued: one per component.
    #[must_use]
    pub const fn gets(&self) -> usize {
        self.gets
    }

    /// How many objects the composite collapses.
    #[must_use]
    pub const fn components(&self) -> usize {
        self.components
    }

    /// The manifest's size.
    ///
    /// ⚠️ **The only bytes this operation wrote**, which is the claim `M5.8`'s
    /// acceptance asks a test to observe. It is a function of the component
    /// count and their region counts, never of the records they hold.
    #[must_use]
    pub const fn manifest_bytes(&self) -> usize {
        self.manifest_bytes
    }
}

/// Collapses `components` into one composite object at `output`.
///
/// Reads each component's footer to learn its regions, writes the manifest,
/// and moves no record bytes.
///
/// ⚠️ **The whole object per component, as `merge` does.** A footer sits at an
/// object's tail and a ranged read of the tail would be one request either
/// way; reading whole is what `merge` already does for the same objects, and
/// the caller that composes today is the caller that considered merging.
///
/// ⚠️ **Sorted by base offset, and duplicates refused, for the reasons
/// `merge` sorts and `tiling()` refuses.** `locate` returns a partition's runs
/// in manifest order, so a caller's list order would decide what order a
/// fetch reads its own records in; and a manifest naming one object twice
/// resolves to two runs over the same bytes, which a fetch counts twice and a
/// retention pass believes.
///
/// ⚠️ **The ordering is the *supplied refs'* order, and that is a
/// per-partition guarantee for the partition those refs describe — not for
/// every partition a component happens to hold.** An [`ObjectRef`] carries one
/// `(object, partition)` span's base offset; a component's footer names every
/// partition in that object, and the manifest copies all of them. So two
/// components ordered correctly for partition 0 may be listed in the wrong
/// order for partition 1, and nothing here can see it: a `Region` carries a
/// byte range inside its object and **no offset in the partition**. Closing it
/// means the manifest carrying a base offset per region, which is `M5.59`.
///
/// # Errors
///
/// Whatever the store says, `parse_footer`'s own errors for a component whose
/// tail is not a bundle,
/// [`Error::IndexObjectMismatch`](oqueue_core::Error::IndexObjectMismatch) for
/// a repeated object, and
/// [`Error::EmptyBundle`](oqueue_core::Error::EmptyBundle) for an empty
/// component list — a composite naming nothing is an object a reader would
/// resolve to no records at all, which is indistinguishable from a composite
/// whose components were all deleted.
pub async fn compose<S>(
    store: &S,
    components: &[ObjectRef],
    output: &ObjectKey,
) -> Result<ComposeOutcome>
where
    S: ObjectStore + ?Sized,
{
    let mut builder = CompositeBuilder::new();
    let mut gets = 0_usize;
    let mut ordered: Vec<&ObjectRef> = components.iter().collect();
    ordered.sort_by_key(|reference| reference.base_offset());
    for reference in ordered {
        let bytes = store
            .get(reference.object(), oqueue_core::ByteRange::Full)
            .await?;
        gets += 1;
        let regions = parse_footer(&bytes, bytes.len() as u64)?;
        builder.push(reference.object().clone(), regions)?;
    }
    let count = builder.len();
    let manifest = builder.seal()?;
    let manifest_bytes = manifest.len();
    store.put(output, manifest, None).await?;
    Ok(ComposeOutcome {
        gets,
        components: count,
        manifest_bytes,
    })
}

/// Reads a composite back and says where one partition's records are.
///
/// ⚠️ **One GET of the manifest, then one ranged GET per run**, which is the
/// indirection a composite costs. The manifest is what a coordinator would
/// hold rather than re-read, and this is the uncached path.
///
/// # Errors
///
/// Whatever the store says, and `parse_composite`'s own errors.
pub async fn read_composite<S>(store: &S, key: &ObjectKey) -> Result<Vec<Component>>
where
    S: ObjectStore + ?Sized,
{
    let bytes = store.get(key, oqueue_core::ByteRange::Full).await?;
    oqueue_core::parse_composite(&bytes)
}
