//! Applying a batch's effects to a copy, in log order, before any of it lands.
//!
//! ⚠️ **One walk, not a check pass mirroring an apply pass.** Publication and
//! the compaction swap were each checked against the *pre-batch* index and
//! then applied to the live one, which is two descriptions of the same fold
//! that have to agree. They did not: `M5.13`'s first round measured two swaps
//! in one batch both accepted (and refused one page at a time), a swap batched
//! with a publication for the same partition leaving a manifest and a
//! reference covering the same offsets, and a chained pair accepted in two
//! pages and refused in one. `M5.62` spent four rounds on the same class for
//! publication alone.
//!
//! So the effects are applied to a **clone of each partition they touch**, in
//! the order the log holds them, and the clone is installed only if every step
//! succeeded. A check that is the fold cannot disagree with the fold.
//!
//! ⚠️ **Only partitions an effect touches are cloned.** A produce batch
//! carries commits and no effects, so it clones nothing — which matters,
//! because a partition's history is thousands of references at doc 14 §3's
//! working set (`ADR-0043`) and the commit path is the coordinator's ack path.
//! Compaction batches pay the copy, and they are rare by construction.

use std::collections::HashMap;

use crate::{Error, ObjectKey, ObjectRef, Offset, PartitionId, Result, TailEntry, TopicId, covers};

use super::{IndexState, partition::PartitionIndex};

/// What one record does to one partition, beyond appending to its tail.
pub(super) enum Effect<'a> {
    /// A manifest now covers everything below `upto`.
    Published {
        manifest: &'a ObjectKey,
        upto: Offset,
    },
    /// A run of history references becomes another run.
    Swapped {
        retiring: &'a [ObjectRef],
        installing: &'a [ObjectRef],
    },
}

/// One effect, with the partition it lands on and how much of its own batch it
/// may see.
///
/// ⚠️ **Preceding, not all of it.** A commit arriving *after* an effect in the
/// log is not part of the state that effect was written against. Counting one
/// made the same log fold one way whole and another way entry-at-a-time, with
/// one broker materializing the index and another refusing it — `M5.62`'s
/// fourth round measured that on publication, and a swap needs it for the same
/// reason: enough commits demote an object into history, which is the only
/// tier a swap may name.
pub(super) struct Staged<'a> {
    pub(super) topic: &'a TopicId,
    pub(super) partition: PartitionId,
    pub(super) effect: Effect<'a>,
    pub(super) precedes: usize,
}

/// The staged tail entries a batch adds, per partition, with each partition's
/// running end offset.
pub(super) type StagedSpans<'a> = HashMap<(&'a TopicId, PartitionId), (Offset, Vec<TailEntry>)>;

/// Every partition an effect touched, as it will be.
pub(super) type Projected<'a> = HashMap<(&'a TopicId, PartitionId), PartitionIndex>;

impl IndexState {
    /// Applies the batch's effects to copies, refusing the whole batch if any
    /// one of them cannot be applied.
    ///
    /// ⚠️ **Nothing here touches `self`** — guarantee 2. What comes back is
    /// what the caller installs once every fallible step has run.
    ///
    /// # Errors
    ///
    /// [`Error::ManifestDoesNotMeetHistory`] for a manifest that does not end
    /// where the partition's remaining history begins, and
    /// [`Error::IndexObjectMismatch`] for a swap whose outputs do not cover
    /// its inputs or that names a reference the partition does not hold in
    /// history. [`Error::OffsetOverflow`] if a stored entry's own fold was
    /// wrong.
    pub(super) fn project<'a>(
        &self,
        effects: &[Staged<'a>],
        staged: &StagedSpans<'a>,
    ) -> Result<Projected<'a>> {
        let mut projected: Projected<'a> = HashMap::new();
        // How many of each partition's staged entries the projection has
        // already taken, so an effect sees the commits before it and no more.
        let mut taken: HashMap<(&TopicId, PartitionId), usize> = HashMap::new();
        for effect in effects {
            let key = (effect.topic, effect.partition);
            let slot = projected.entry(key).or_insert_with(|| {
                self.partition(effect.topic, effect.partition)
                    .cloned()
                    .unwrap_or_default()
            });
            let already = taken.entry(key).or_insert(0);
            catch_up(slot, staged.get(&key), already, effect.precedes);
            apply_effect(slot, &effect.effect)?;
        }
        // The commits *after* the last effect, so an installed projection is
        // the whole batch rather than its prefix.
        for (key, slot) in &mut projected {
            let already = taken.entry(*key).or_insert(0);
            let all = staged.get(key).map_or(0, |(_, entries)| entries.len());
            catch_up(slot, staged.get(key), already, all);
            if let Some((end, _)) = staged.get(key) {
                slot.end_offset = *end;
            }
        }
        Ok(projected)
    }
}

/// Pushes this partition's staged entries up to `upto`, tracking how many have
/// gone in so no entry is pushed twice.
///
/// ⚠️ **No bound check against the vector, because `upto` cannot exceed it.**
/// It is either an effect's `precedes` — the length of this partition's staged
/// entries when that effect was *read*, and the vector only grows after — or
/// that length itself, on the pass that takes the commits after the last
/// effect. A second condition would be a branch no input can take, which this
/// repository holds to be worse than no branch; cargo-mutants found the
/// version that had one, by showing both spellings behave identically.
fn catch_up(
    slot: &mut PartitionIndex,
    staged: Option<&(Offset, Vec<TailEntry>)>,
    already: &mut usize,
    upto: usize,
) {
    let Some((_, entries)) = staged else {
        return;
    };
    while *already < upto {
        slot.push(entries[*already].clone());
        *already += 1;
    }
}

/// One effect against one partition, as it now stands.
fn apply_effect(slot: &mut PartitionIndex, effect: &Effect<'_>) -> Result<()> {
    match effect {
        Effect::Published { manifest, upto } => {
            if let Some(expected) = slot.boundary(*upto) {
                return Err(Error::ManifestDoesNotMeetHistory {
                    upto: upto.get(),
                    expected: expected.get(),
                });
            }
            slot.absorb((*manifest).clone(), *upto);
        }
        Effect::Swapped {
            retiring,
            installing,
        } => {
            let from: Vec<&ObjectRef> = retiring.iter().collect();
            let to: Vec<&ObjectRef> = installing.iter().collect();
            covers(&from, &to)?;
            for reference in *retiring {
                if !slot.holds_in_history(reference) {
                    return Err(Error::IndexObjectMismatch);
                }
            }
            slot.replace_history(retiring, installing);
        }
    }
    Ok(())
}
