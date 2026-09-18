//! Reading a batch into its own scratch, and applying it to a copy in log
//! order before any of it lands.
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

use crate::{
    CommittedSpan, Error, ObjectKey, ObjectRef, Offset, PartitionId, Result, TailEntry, TopicId,
    covers,
};

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
    /// Stages one span's contribution, without touching `self`.
    ///
    /// ⚠️ The base offset comes from the **staged** running value first and
    /// only then from what is already committed, which is what makes a
    /// partition appearing twice in one batch accumulate rather than restart.
    /// `M3.8` commits every N ≥ 1,000 entries, so that is its ordinary case.
    pub(super) fn stage_span<'a>(
        &self,
        staged: &mut HashMap<(&'a TopicId, PartitionId), (Offset, Vec<TailEntry>)>,
        object: &ObjectKey,
        span: &'a CommittedSpan,
    ) -> Result<()> {
        // ⚠️ **A span of no records is refused** (`M5.77`). It would become an
        // `ObjectRef` covering no offsets, which `M5.13` made *un-retirable*:
        // `contiguous_span` refuses any set containing one, so no
        // `RangeCompacted` can ever name it and the entry is permanent. It
        // would also sort into history ahead of the real object at its base
        // offset, where a fetch resuming there skips an acknowledged record —
        // the defect `M5.13`'s third round measured on the installing side.
        //
        // ⚠️ **No flush produces one** — `BundleBuilder::push` refuses a
        // zero-count region — and that is exactly why this is here: the
        // index's job is to refuse a malformed log, not to trust the writer,
        // which is the standard every other check in this fold is held to.
        if span.record_count() == 0 {
            return Err(Error::EmptySpanInLog {
                topic: span.topic().as_str().to_owned(),
                partition: span.partition().get(),
                object: object.as_str().to_owned(),
            });
        }
        let key = (span.topic(), span.partition());
        let base = staged
            .get(&key)
            .map(|(end, _)| *end)
            .or_else(|| {
                self.partitions
                    .get(span.topic())
                    .and_then(|parts| parts.get(&span.partition()))
                    .map(|p| p.end_offset)
            })
            .unwrap_or(Offset::ZERO);
        // ⚠️ `Offset::add`, not `+`: a wrapped offset is smaller than the one
        // before it, and every later comparison is then wrong.
        let next = base.add(i64::from(span.record_count()))?;
        let entry = TailEntry::new(
            ObjectRef::new(object.clone(), base, span.record_count()),
            span.bytes(),
        );
        let slot = staged.entry(key).or_insert((base, Vec::new()));
        slot.0 = next;
        slot.1.push(entry);
        Ok(())
    }

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
    /// [`Error::SwapRefused`] for a swap whose outputs do not cover
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
            apply_effect(slot, effect.topic, effect.partition, &effect.effect)?;
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
///
/// ⚠️ **Every refusal here names the `(topic, partition)` the record does**
/// (`M5.79`). The fold's caller is a broker materializing a log it did not
/// write, and the failure is permanent — so a message that identifies no
/// entry leaves an operator bisecting a log to find the one that does not fit.
fn apply_effect(
    slot: &mut PartitionIndex,
    topic: &TopicId,
    partition: PartitionId,
    effect: &Effect<'_>,
) -> Result<()> {
    match effect {
        Effect::Published { manifest, upto } => {
            if let Some(expected) = slot.boundary(*upto) {
                return Err(Error::ManifestDoesNotMeetHistory {
                    topic: topic.as_str().to_owned(),
                    partition: partition.get(),
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
            // ⚠️ **`covers` keeps its own vocabulary and this translates it.**
            // That function is shared with `oqueue-compact`'s merge, where
            // `IndexObjectMismatch` is the right thing to say; what it cannot
            // know is which record asked.
            covers(&from, &to).map_err(|error| match error {
                Error::IndexObjectMismatch => Error::SwapRefused {
                    topic: topic.as_str().to_owned(),
                    partition: partition.get(),
                    because: why_it_does_not_cover(retiring, installing),
                },
                other => other,
            })?;
            for reference in *retiring {
                if !slot.holds_in_history(reference) {
                    return Err(Error::SwapRefused {
                        topic: topic.as_str().to_owned(),
                        partition: partition.get(),
                        because: format!(
                            "it retires {}, which this partition's history does not hold",
                            reference.object().as_str()
                        ),
                    });
                }
            }
            slot.replace_history(retiring, installing);
        }
    }
    Ok(())
}

/// How many of this partition's entries the batch has staged so far.
///
/// ⚠️ **So far, not in total.** An effect sees the commits before it in the
/// log and no others; a commit after it is not part of the state it was
/// written against. `M5.62`'s fourth round measured what counting the whole
/// batch does.
pub(super) fn precedes(staged: &StagedSpans<'_>, topic: &TopicId, partition: PartitionId) -> usize {
    staged
        .get(&(topic, partition))
        .map_or(0, |(_, entries)| entries.len())
}

/// Why a swap's two runs did not cover each other, in a sentence.
///
/// ⚠️ **It describes, it does not decide** (`M5.79`). The decision is
/// [`covers`](crate::covers)'s and stays there — this is reached only after
/// that function has already said no, so the two cannot disagree about
/// accepting or refusing. Re-deriving the rule here would be the second
/// description of one rule that `M5.13`'s first round found three defects in.
///
/// ⚠️ **What it adds is the one cause an operator cannot see in the offsets.**
/// A reference covering no offsets passes a comparison of bounds — `[0,1)`
/// against `[0,1)` — and fails anyway, so a message about coverage would send
/// a reader to check the one thing that is not wrong. An empty side is visible
/// in the record, but naming it is what stops it reading as a bounds mismatch.
///
/// ⚠️ **"The bounds differ" is the fallback and is true of what is left.** A
/// gap, an overlap and a run that starts or ends elsewhere are all visible in
/// the offsets the record carries, so naming them individually would buy an
/// operator nothing a diff of the two lists does not already show.
fn why_it_does_not_cover(retiring: &[ObjectRef], installing: &[ObjectRef]) -> String {
    // ⚠️ Destructured rather than defaulted, so every arm below is one some
    // input reaches: `retiring` is non-empty past this point, which is what
    // lets the fallback name the run it is about.
    let Some(first) = retiring.first() else {
        return format!(
            "it retires nothing and installs {}, and neither side of a swap may be empty",
            installing.len()
        );
    };
    if installing.is_empty() {
        return format!(
            "it retires {} reference(s) starting at {} and installs nothing, and neither \
             side of a swap may be empty",
            retiring.len(),
            first.object().as_str()
        );
    }
    if let Some(empty) = retiring
        .iter()
        .chain(installing.iter())
        .find(|reference| reference.record_count() == 0)
    {
        return format!(
            "it names {}, which covers no offsets — such a reference can never be \
             retired, and sorts ahead of the real object at its base offset",
            empty.object().as_str()
        );
    }
    format!(
        "the {} reference(s) it installs do not cover exactly the {} it retires, \
         starting at {}",
        installing.len(),
        retiring.len(),
        first.object().as_str()
    )
}
