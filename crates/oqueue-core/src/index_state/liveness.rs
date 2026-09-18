//! Which objects the index still names: the count a deleter asks before it
//! deletes (`ADR-0045`, `M5.20`).
//!
//! ⚠️ **Maintained per changed entry, never by walking the index.** A commit
//! adds one per entry it stages, a swap subtracts one per retired reference
//! and adds one per installed, a trim subtracts one per entry it drops. An
//! object at zero is named by nothing the index can serve.
//!
//! ⚠️ **A publication subtracts nothing, deliberately.** A manifest absorbs
//! history entries, and the data objects it absorbed are still referenced —
//! by the manifest's contents, which this fold does not hold and must not
//! (`ADR-0042`). So an absorbed object's count stays up for good: storage
//! leaks, and no byte a reader could reach is ever released. That is the
//! direction FR-35 permits; releasing them needs the manifest read back.

use std::collections::HashMap;

use super::IndexState;
use super::projection::StagedSpans;
use crate::ObjectKey;

/// One change a batch makes to an object's count.
#[derive(Debug)]
pub(super) enum RefChange {
    /// A reference to this object was installed.
    Added(ObjectKey),
    /// A reference to this object was dropped.
    Dropped(ObjectKey),
}

/// Every staged entry names its object once, whichever path installs it.
pub(super) fn stage(staged: &StagedSpans<'_>, changes: &mut Vec<RefChange>) {
    for (_, entries, _) in staged.values() {
        changes.extend(
            entries
                .iter()
                .map(|entry| RefChange::Added(entry.reference().object().clone())),
        );
    }
}

/// How many index entries name each object.
#[derive(Debug, Default)]
pub(super) struct References(HashMap<ObjectKey, usize>);

impl References {
    /// Applies a batch's changes: every addition first, then every drop.
    ///
    /// ⚠️ **Additions first**, so a trim in the same batch as the commits it
    /// trims nets to zero rather than dropping below it on the way.
    pub(super) fn apply(&mut self, changes: Vec<RefChange>) {
        let (added, dropped): (Vec<_>, Vec<_>) = changes
            .into_iter()
            .partition(|change| matches!(change, RefChange::Added(_)));
        for change in added.into_iter().chain(dropped) {
            match change {
                RefChange::Added(key) => *self.0.entry(key).or_insert(0) += 1,
                RefChange::Dropped(key) => {
                    if let Some(count) = self.0.get_mut(&key) {
                        *count = count.saturating_sub(1);
                        if *count == 0 {
                            self.0.remove(&key);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn get(&self, object: &ObjectKey) -> usize {
        self.0.get(object).copied().unwrap_or(0)
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

impl IndexState {
    /// How many index entries name `object` (`ADR-0045`).
    ///
    /// ⚠️ **Zero is the precondition for deleting it, not the permission.**
    /// A reader holding a reference fetched before the count reached zero can
    /// still read it; the lifecycle's delay is what covers that reader
    /// (FR-35's inequality, `M5.22`). ⚠️ **An object absorbed into a partition
    /// manifest never reaches zero here**, because the manifest still names it
    /// and this fold does not read manifests.
    #[must_use]
    pub fn references(&self, object: &ObjectKey) -> usize {
        self.references.get(object)
    }
}
