//! What the model holds between requests, as distinct from what it answers.
//!
//! ⚠️ **Split from `model.rs` at `M10.7`, the second time that file reached
//! `code-structure.md`'s 500-line limit in one row** — the first produced
//! `transport.rs`. The seam is the same shape as that one: `model.rs` is the
//! behaviour, and this is the data it is a function of. A fault, a latency
//! source and a one-shot flag all landed here in the same milestone, which is
//! the growth that made the seam worth having.
//!
//! ⚠️ **The fields are `pub(crate)` rather than private**, which they were
//! while they lived beside their only reader. That is the cost of the split
//! and it is the reason the split is here and not somewhere that would have
//! cost more: `sim` is a private module tree inside one test binary, so the
//! widest this reaches is the file next door.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used, clippy::unwrap_used)]
// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the bare
// `pub` clippy's `redundant_pub_crate` asks for — the trade every file in this
// tree makes, for the same reason.
#![allow(clippy::redundant_pub_crate)]

use std::collections::HashMap;

use super::faults::Faults;

/// ⚠️ **Fixed, because this file is the deterministic one.** A real
/// `Last-Modified` would make two runs of one seed differ in a header
/// `object_store` parses, which is the whole property `M10.4` is built on.
pub(crate) const LAST_MODIFIED: &str = "Wed, 01 Jan 2020 00:00:00 GMT";

/// What the model holds under one key.
#[derive(Clone)]
pub(crate) struct Stored {
    pub(crate) bytes: Vec<u8>,
    /// The entity tag this object would present. ⚠️ A counter rather than a
    /// hash: two writes of identical bytes must produce **different** tags, or
    /// `Precondition::IfMatches` would admit a write it should refuse.
    pub(crate) etag: u64,
}

#[derive(Default)]
pub(crate) struct ModelState {
    pub(crate) objects: HashMap<String, Stored>,
    pub(crate) next_etag: u64,
    /// Per-request delay, when the model was built with one.
    ///
    /// ⚠️ **Off by default**, so every test written before `M10.6` keeps its
    /// meaning: a conformance run that suddenly took simulated minutes would
    /// be a different test, not the same one made realistic.
    pub(crate) latency: Option<super::latency::Latency>,
    /// One-shot: the next PUT stores its bytes and then answers a failure.
    ///
    /// ⚠️ **The thing a real S3 cannot be told to do**, which is why
    /// `s3_minio.rs` declares `injectable_ack_loss: false` and skips
    /// `a_failed_put_is_not_proof_of_absence` — `ADR-0005` guarantee 2, that a
    /// failed write is not proof of absence. A model can be told, so the
    /// simulated backend runs the case S3 must skip.
    pub(crate) lose_next_ack: bool,
    /// What this model has been told to do wrong. ⚠️ **Empty by default**, on
    /// the same argument as `latency`: a conformance run that started
    /// answering 503s would be a different test rather than the same one made
    /// realistic.
    pub(crate) faults: Faults,
}

impl std::fmt::Debug for ModelState {
    /// ⚠️ **Counts, not contents.** A `Debug` that printed every stored object
    /// would put an arbitrary payload in a panic message; the count is what a
    /// reader of a failure actually needs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelState")
            .field("objects", &self.objects.len())
            .field("next_etag", &self.next_etag)
            .field("lose_next_ack", &self.lose_next_ack)
            .field("faults", &self.faults)
            .field("latency", &self.latency)
            .finish()
    }
}
