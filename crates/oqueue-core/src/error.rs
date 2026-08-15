//! The crate's error type.
//!
//! `error-handling.md` rule 3 puts one error enum per crate and builds it with
//! `thiserror`; rule 5 says it classifies rather than wraps. ⚠️ This is the
//! *start* of that taxonomy, not the whole of it — `M0.6` adds the redaction
//! rules FR-44 needs, and the variants below are the ones `M0.5`'s four types
//! can actually produce.

/// Everything `oqueue-core` can fail at.
///
/// ⚠️ **Deliberately not `#[non_exhaustive]`.** That attribute would force every
/// downstream `match` to carry a `_ =>` arm, which `rust-style.md` rule 16 bans
/// precisely because it absorbs a new variant silently — and a new variant here
/// is a new failure mode every caller should be made to consider. Nothing in
/// this workspace is published, so the compatibility the attribute buys is
/// compatibility with nobody. The cost is real and is the point: adding a
/// variant breaks every exhaustive match, which is the compiler doing the
/// review.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A topic identifier was empty.
    #[error("topic id is empty")]
    EmptyTopicId,

    /// A partition index was negative.
    #[error("partition id must not be negative, got {got}")]
    NegativePartitionId {
        /// The rejected value.
        got: i32,
    },

    /// An offset was negative.
    #[error("offset must not be negative, got {got}")]
    NegativeOffset {
        /// The rejected value.
        got: i64,
    },

    /// Offset arithmetic was asked to move backwards.
    ///
    /// ⚠️ Distinct from [`Error::NegativeOffset`], and the distinction is the
    /// point: a negative *offset* means a peer or a decoder got it wrong, while
    /// a negative *delta* means this project's own sequencing did. Collapsing
    /// them into one variant would leave a caller unable to tell which.
    #[error("cannot advance an offset by a negative amount, got {got}")]
    NegativeOffsetDelta {
        /// The rejected delta.
        got: i64,
    },

    /// Offset arithmetic would have exceeded the protocol's `int64` range.
    ///
    /// Returned instead of wrapping. M3's offset sequencing rests on
    /// monotonicity, and a wrap is the one arithmetic result that breaks it
    /// silently.
    #[error("offset {base} + {delta} exceeds the maximum offset")]
    OffsetOverflow {
        /// The offset the addition started from.
        base: i64,
        /// The amount that could not be added.
        delta: i64,
    },

    /// An object key was empty.
    #[error("object key is empty")]
    EmptyObjectKey,
}

/// The crate's result alias.
pub type Result<T> = core::result::Result<T, Error>;
