//! What a merge did, and what the commit after it needs.
//!
//! ⚠️ **Its own module because the account of a merge outlives the merge**
//! (`code-structure.md` rule 18): `M5.13`'s atomic commit consumes this, and
//! `merge.rs` reached the 500-line limit holding both the run and its record.

use oqueue_core::{CommittedSpan, ObjectKey, Written};

/// What a merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub(crate) gets: usize,
    pub(crate) puts: usize,
    pub(crate) records: i64,
    pub(crate) spans: Vec<CommittedSpan>,
    pub(crate) written: Written,
    pub(crate) object: Option<ObjectKey>,
}

impl MergeOutcome {
    /// An empty run: no reads, no write, no object.
    ///
    /// ⚠️ **The only constructor, and it takes nothing**, which is what keeps
    /// the numbers honest: the fields are `pub(crate)` so the two functions
    /// that actually merge can fill them in, and nothing outside this crate
    /// can report a run it did not make. A `new` taking six numbers was the
    /// same hazard with a longer signature.
    pub(crate) fn empty() -> Self {
        Self {
            gets: 0,
            puts: 0,
            records: 0,
            spans: Vec::new(),
            written: Written::default(),
            object: None,
        }
    }

    /// The object this run sealed, or `None` for an empty round.
    ///
    /// ⚠️ **The caller cannot know it any other way, now that the run mints
    /// it** (`M5.75`), and the commit that follows needs it: the `ObjectRef`
    /// a swap installs names this key. ⚠️ **`None` is a round that wrote
    /// nothing**, not a round whose key was lost — an empty round writes no
    /// object and consumes no sequence number, so there is no key to report.
    #[must_use]
    pub const fn object(&self) -> Option<&ObjectKey> {
        self.object.as_ref()
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
