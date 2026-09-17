//! What a merge did, and what the commit after it needs.
//!
//! ⚠️ **Its own module because the account of a merge outlives the merge**
//! (`code-structure.md` rule 18): `M5.13`'s atomic commit consumes this, and
//! `merge.rs` reached the 500-line limit holding both the run and its record.

use oqueue_core::{CommittedSpan, Written};

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub(super) gets: usize,
    pub(super) puts: usize,
    pub(super) records: i64,
    pub(super) spans: Vec<CommittedSpan>,
    pub(super) written: Written,
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
        written: Written,
    ) -> Self {
        Self {
            gets,
            puts,
            records,
            spans,
            written,
        }
    }

    /// Object reads the merge issued.
    #[must_use]
    pub const fn gets(&self) -> usize {
        self.gets
    }

    /// Object writes the run issued: **one for a merge or a non-empty round,
    /// and zero for an empty round**, which writes nothing rather than an
    /// empty object. `CostEstimate::puts` is per plan, so the two agree only
    /// for a single-plan merge.
    #[must_use]
    pub const fn puts(&self) -> usize {
        self.puts
    }

    /// What the write actually cost: bytes moved, and parts written.
    ///
    /// ⚠️ **Measured here because no estimate can carry it** (`ADR-0039`).
    /// `CostEstimate` is denominated in records, because the index holds no
    /// byte length for the history tier and a byte-denominated estimate would
    /// need a GET per candidate — the very cost it exists to bound. So the
    /// egress a client-side copy pays and the part count a multipart write
    /// takes are reported after the fact rather than predicted before it.
    ///
    /// ⚠️ **A round's *request* count is still derivable from nothing here.**
    /// A backend's create and complete are issued inside the writer, so a real
    /// S3 write costs two more requests than it writes parts;
    /// `roadmap.md`'s "Multipart's true per-request cost" row (M14, from M1)
    /// is what makes that observable.
    #[must_use]
    pub const fn written(&self) -> Written {
        self.written
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
