//! What sequencing a commit can fail at, and whether retrying is worth it.

use oqueue_core::{Error, MaterializedIndex};

/// Everything `oqueue-coordinator` can fail at.
///
/// ⚠️ **Its own enum rather than `oqueue_core::Error`** — `error-handling.md`
/// rule 3 and `contracts.md` rule 17. Two of the three failures below arrive
/// *as* an `oqueue_core::Error` from a seam underneath, and re-throwing them
/// unchanged is what rule 10 forbids: a caller holding a bare
/// [`Error::Transient`] cannot tell a journal that may succeed on retry from a
/// position that can never be assigned, and those have opposite answers.
///
/// ⚠️ **Deliberately not `#[non_exhaustive]`**, for the reason
/// [`oqueue_core::Error`] is not: a new variant is a new failure mode every
/// caller should be made to consider, and a `_ =>` arm absorbing it silently is
/// the bug that would not surface until a produce answered the wrong thing.
///
/// # Is retrying worth it
///
/// `error-handling.md` rule 7's question, answered per variant rather than left
/// to a caller reading a message string:
///
/// | Variant | Retry |
/// |---|---|
/// | [`Unavailable`](Self::Unavailable) | Never — this coordinator is done. |
/// | [`Unassignable`](Self::Unassignable) | Never — the arithmetic will not change. |
/// | [`Journal`](Self::Journal) | The inner error's class decides. |
/// | [`Unreplayable`](Self::Unreplayable) | Never — the same log refuses the same way. |
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoordinatorError {
    /// The coordinator's serializing loop is no longer accepting commits.
    ///
    /// Its last handle was dropped, or the process is shutting down. ⚠️ The
    /// caller answers with the unset sentinel
    /// ([`UNASSIGNED_OFFSET`](crate::UNASSIGNED_OFFSET)) and never with an
    /// offset it invented to fill the hole.
    #[error("the coordinator is not accepting commits")]
    Unavailable,

    /// The commit could not be given a position at all.
    ///
    /// A partition's offset line, or the log's version line, would have had to
    /// leave its range — [`Error::OffsetOverflow`] or
    /// [`Error::CommitVersionOverflow`]. ⚠️ Detected **before** the journal
    /// step, so nothing was written and nothing was consumed.
    #[error("the commit could not be assigned a position: {0}")]
    Unassignable(#[source] Error),

    /// The position was computed but could not be made durable.
    ///
    /// ⚠️ **Nothing was consumed by the attempt.** The metadata log's own
    /// contract is that a refused append stores nothing, and this coordinator
    /// applies a staged commit to its allocator only after the append
    /// resolves `Ok` — so the retry gets the same version and the same
    /// offsets the refused attempt would have had.
    #[error("the assignment could not be journaled: {0}")]
    Journal(#[source] Error),

    /// The log handed to [`Coordinator::open`](crate::Coordinator::open)
    /// holds an entry the replay cannot fold (`M6.3`).
    ///
    /// ⚠️ **Refused rather than served around.** A replay that skipped an
    /// entry would resume the offset line short of where the log says it
    /// reached, and the next commit would reuse offsets already acknowledged —
    /// silent duplication, which a refusal at startup is always better than.
    #[error("the metadata log cannot be replayed at version {version}: {source}")]
    Unreplayable {
        /// The entry the replay stopped at.
        version: u64,
        /// Why the fold refused it.
        #[source]
        source: Error,
    },
}

/// A refused [`open`](crate::Coordinator::open), and the index it was given.
///
/// ⚠️ **The index comes back, and that is the whole type.** `open` takes a
/// `Box<dyn MaterializedIndex>` by value — `ADR-0024`, so the caller keeps no
/// writable handle — and a plain `Err` therefore *destroys* it. For M3's
/// `MemoryIndex` that costs an allocation; for doc 10 #12's engine it is an
/// open database, and a caller that hit [`CoordinatorError::Journal`] on a
/// transient log read could not retry without building one again.
///
/// ⚠️ **Every refusal returns it, because every refusal precedes the clear.**
/// `open` reads the log, refuses a non-empty one, and only then touches the
/// index — so what comes back is exactly what went in, unmodified, on both
/// paths.
///
/// ⚠️ **The case that makes it more than a convenience is
/// [`Journal`](CoordinatorError::Journal)**, whose inner error decides whether
/// to retry: a transient log read is the one refusal where the right next move
/// is the *same call again*, and a caller cannot make it without the index.
/// ⚠️ **Not [`Unreplayable`](CoordinatorError::Unreplayable)**, though the
/// index comes back there too — that one is `Never`-retryable, and the index
/// comes back *cleared*, because a replay that stopped partway had folded
/// part of the log into it.
#[derive(Debug)]
pub struct OpenRejected {
    error: CoordinatorError,
    index: Box<dyn MaterializedIndex>,
}

impl OpenRejected {
    pub(crate) const fn new(error: CoordinatorError, index: Box<dyn MaterializedIndex>) -> Self {
        Self { error, index }
    }

    /// Why the open was refused.
    #[must_use]
    pub const fn error(&self) -> &CoordinatorError {
        &self.error
    }

    /// The reason and the index, for a caller that wants both.
    #[must_use]
    pub fn into_parts(self) -> (CoordinatorError, Box<dyn MaterializedIndex>) {
        (self.error, self.index)
    }

    /// The index alone, for a caller that has already read the reason.
    #[must_use]
    pub fn into_index(self) -> Box<dyn MaterializedIndex> {
        self.index
    }
}

impl core::fmt::Display for OpenRejected {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for OpenRejected {
    /// ⚠️ **The reason's own source, not the reason.** `Display` already
    /// forwards to `self.error`, so sourcing it too would make the top of a
    /// chain and its first cause the same string — one incident that reads as
    /// two. This envelope adds a *field*, not a layer, which is
    /// `thiserror`'s `#[error(transparent)]` semantics written out. ⚠️ **No
    /// standard rule governs this**; it is a judgement, recorded here because
    /// the next envelope type will face it.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{CoordinatorError, OpenRejected};
    use oqueue_core::{Error, FakeMaterializedIndex};
    use std::error::Error as _;

    fn rejected() -> OpenRejected {
        OpenRejected::new(
            CoordinatorError::Journal(Error::Transient),
            Box::new(FakeMaterializedIndex::new()),
        )
    }

    /// ⚠️ **A wrapper that swallowed its reason would be worse than no
    /// wrapper.** `OpenRejected` exists to carry the index back; what an
    /// operator sees is still the error, so its `Display` has to be the
    /// error's and not a second, emptier message. `bin/oqueue`'s composition
    /// root formats it straight into an `io::Error`, so this string is what
    /// reaches an operator's terminal on a broker that will not start.
    #[test]
    fn a_rejection_reads_as_the_error_it_carries() {
        let rejected = rejected();
        assert_eq!(rejected.to_string(), rejected.error().to_string());
        assert!(
            !rejected.to_string().is_empty(),
            "a refusal that renders as nothing tells an operator nothing"
        );
    }

    /// ⚠️ **This envelope adds a field, not a layer**, so its `source` is the
    /// *reason's* source. A `source` returning the reason itself would make
    /// the top of a chain and its first cause the same string, which reads as
    /// two incidents.
    ///
    /// ⚠️ **It does not make the chain repetition-free**, and claiming so
    /// would be claiming a property of `CoordinatorError` this type does not
    /// have: `Journal` interpolates its source into its own message *and*
    /// marks it `#[source]`, so "transient backend failure" still appears at
    /// two depths. That is the enum's shape and predates this envelope; what
    /// is asserted here is only that the envelope adds no third copy.
    #[test]
    fn a_rejection_adds_no_layer_of_its_own_to_the_chain() {
        let rejected = rejected();
        let source = rejected
            .source()
            .expect("a journal failure has the store error under it");
        assert_eq!(source.to_string(), Error::Transient.to_string());
        assert_ne!(
            source.to_string(),
            rejected.to_string(),
            "the envelope must not render its own reason as its cause"
        );
    }
}
