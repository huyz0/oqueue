//! What a compaction run's output is called.
//!
//! ⚠️ **Its own module because naming is not the merge** (`code-structure.md`
//! rule 18), the same split `oqueue-core` makes between `bundle.rs` and
//! `bundle_name.rs` — and `support.rs` was at the 500-line limit, which is
//! rule 18's signal rather than a formatting problem.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is the only root of this
// test binary and nothing outside it can name this. `support.rs` makes the
// same call for the same reason.
#![allow(unreachable_pub)]

use oqueue_compact::CompactionNamer;

/// A namer for a test's compaction run. ⚠️ One per *run*: the keys are
/// unrepeatable only because a sequence advances, so binding this once per
/// attempt would hand every attempt the same key.
pub fn namer() -> CompactionNamer {
    CompactionNamer::new("test-writer").expect("a valid writer identity")
}

/// ⚠️ **A key nothing else in this repository can produce.** Collision-freedom
/// across the two writers rests on the prefixes being disjoint, not on two
/// independently minted identity spaces never overlapping — which nothing
/// checks and nobody would notice failing.
#[test]
fn a_compacted_key_is_not_a_key_a_flush_could_write() {
    let mut namer = namer();
    let first = namer.next_key().expect("a first key");
    let second = namer.next_key().expect("a second key");

    assert_ne!(first, second, "the sequence advances");
    assert!(
        first.as_str().starts_with("compacted/"),
        "under compaction's own prefix, got {first:?}"
    );
    assert!(
        !first.as_str().starts_with("bundles/"),
        "and never under a flush's"
    );

    assert!(
        CompactionNamer::new("").is_err(),
        "an empty writer identity is no identity"
    );
    assert!(
        CompactionNamer::new("a/b").is_err(),
        "a slash would move the boundary between the writer and its sequence"
    );
}
