//! What sequencing a commit can fail at, and whether retrying is worth it.

use oqueue_core::Error;

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
/// | [`ReplayRequired`](Self::ReplayRequired) | Never — a startup fault, not a request fault. |
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
    /// already holds entries.
    ///
    /// ⚠️ **Refused rather than resumed, and that is the honest failure.** A
    /// coordinator opened over a non-empty log would restart both its version
    /// line and every partition's offset line at zero, which the log's
    /// monotonicity check would catch and a partition's offsets would not —
    /// the second is silent duplication. `M3.8` is the row that replays a log
    /// into the allocator; until it lands, this is a refusal at startup rather
    /// than wrong offsets at run time.
    #[error(
        "the metadata log already holds entries up to version {last_version}; \
         replay is required before positions can be assigned"
    )]
    ReplayRequired {
        /// The highest version already in the log.
        last_version: u64,
    },
}
