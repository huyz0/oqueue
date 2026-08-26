//! The per-process identity every bundled object's name is built on.
//!
//! ⚠️ **Its own module because it is a safety property, not a formatting
//! detail.** `ADR-0026` decided a flush writes with no `Precondition`, which
//! `ADR-0020` point 4 forbids on the offset stream anyway, and a retried flush
//! is safe only because it rewrites *its own* key. `BundleNamer`'s doc states
//! the obligation that rests on and names this milestone as the one that owes
//! it: the identity must be per **process**. Two live brokers sharing one
//! would overwrite each other's objects, and the records lost would be ones
//! already acknowledged.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Distinguishes two identities minted by the same process.
///
/// ⚠️ **Not redundant with the timestamp, and portability is why.** Two mints
/// a few nanoseconds apart are distinct only if the platform clock resolves
/// nanoseconds; Windows' does not (`portability.md` rule 3), so on that
/// platform the timestamp alone would let the broker's own suite build two
/// clusters sharing an identity. This counter makes the property hold on every
/// platform rather than on the one the test happened to run on.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// One process's writer identity.
///
/// ⚠️ **[`mint`](Self::mint) is the only way to make one**, and that is the
/// design rather than an omission. A constructor taking a string would let a
/// node id, a hostname, or a configuration value become an identity — each of
/// which is shared by two brokers on one host, and by a process and its own
/// restart. There is no `From<String>`, no `new(&str)`, and no `Default`, so
/// the unsafe shapes are unrepresentable rather than discouraged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterId(String);

impl WriterId {
    /// Mints an identity for this process.
    ///
    /// ⚠️ **Process id *and* start time, because neither alone is enough.** A
    /// pid distinguishes two brokers running side by side on one host but is
    /// reused after a process exits — and a restart that reused the identity
    /// would restart the sequence at zero and rewrite its predecessor's
    /// objects, which is exactly the case `BundleNamer`'s doc calls out. The
    /// nanosecond stamp distinguishes the restart. Colliding needs the same
    /// pid in the same nanosecond, which needs the clock to have gone
    /// backwards across a process lifetime as well. Within one process the
    /// `MINTED` counter decides, so no clock resolution is relied on.
    ///
    /// ⚠️ **The real clock, deliberately, and not the [`Clock`] seam.** A
    /// faked clock is shared by every broker a test builds, so wiring this to
    /// the seam would make two identities collide precisely in the
    /// configuration written to prove they do not. `check-sans-io.sh` exempts
    /// this crate from the clock pattern for reasons like this one.
    ///
    /// [`Clock`]: oqueue_core::Clock
    #[must_use]
    pub fn mint() -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let nth = MINTED.fetch_add(1, Ordering::Relaxed);
        Self(format!("{:x}-{stamp:x}-{nth:x}", std::process::id()))
    }

    /// The identity as `BundleNamer` takes it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::WriterId;

    /// ⚠️ **Two mints in one process must differ**, because the broker's own
    /// test suite builds several clusters in one process and each writes to
    /// the same store. A pid-only identity passes every other assertion here
    /// and fails this one.
    #[test]
    fn two_identities_minted_in_one_process_are_distinct() {
        let first = WriterId::mint();
        let second = WriterId::mint();
        assert_ne!(first, second);
        // ⚠️ And distinct *as key components*, which is the property that
        // matters: `BundleNamer` sees only this string.
        assert_ne!(first.as_str(), second.as_str());
    }

    /// ⚠️ `BundleNamer::new` refuses a `/`, because it would let two
    /// identities produce one key by moving the boundary between the writer
    /// and its sequence. Nothing this mints may contain one.
    #[test]
    fn a_minted_identity_is_a_key_component_bundle_namer_accepts() {
        let id = WriterId::mint();
        assert!(!id.as_str().is_empty());
        assert!(!id.as_str().contains('/'));
        assert!(oqueue_core::BundleNamer::new(id.as_str()).is_ok());
        // ⚠️ The string is *this* identity's, not a constant: an accessor
        // returning something fixed would pass every assertion above while
        // giving two processes one sequence.
        assert!(
            id.as_str()
                .starts_with(&format!("{:x}-", std::process::id())),
            "the identity names the process that minted it"
        );
    }
}
