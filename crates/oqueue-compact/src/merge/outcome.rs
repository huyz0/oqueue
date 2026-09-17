//! What a merge did, and what the commit after it needs.
//!
//! ⚠️ **Its own module because the account of a merge outlives the merge**
//! (`code-structure.md` rule 18): `M5.13`'s atomic commit consumes this, and
//! `merge.rs` reached the 500-line limit holding both the run and its record.

use oqueue_core::CommittedSpan;

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub(super) gets: usize,
    pub(super) puts: usize,
    pub(super) records: i64,
    pub(super) spans: Vec<CommittedSpan>,
}

impl MergeOutcome {
    /// Builds one.
    ///
    /// ⚠️ **`pub(crate)` rather than public**: the only honest source of these
    /// numbers is a merge that happened, and a constructor anyone could call
    /// would let a caller report a run it did not make.
    pub(crate) const fn new(
        gets: usize,
        puts: usize,
        records: i64,
        spans: Vec<CommittedSpan>,
    ) -> Self {
        Self {
            gets,
            puts,
            records,
            spans,
        }
    }

    /// Object reads the merge issued.
    #[must_use]
    pub const fn gets(&self) -> usize {
        self.gets
    }

    /// Object writes it issued.
    #[must_use]
    pub const fn puts(&self) -> usize {
        self.puts
    }

    /// Records it moved.
    #[must_use]
    pub const fn records(&self) -> i64 {
        self.records
    }

    /// The spans describing the object it wrote.
    ///
    /// ⚠️ **Carried out of the merge rather than re-derived.** `M5.13`'s
    /// atomic commit needs them, and reading them back would mean a GET the
    /// one-GET-per-input claim does not count and a second derivation of a
    /// fact `BundleBuilder::seal` produces once — the drift `bundle.rs` warns
    /// about in as many words.
    #[must_use]
    pub fn spans(&self) -> &[CommittedSpan] {
        &self.spans
    }
}
