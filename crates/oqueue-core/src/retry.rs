//! Whether a failed [`crate::ObjectStore`] call is worth retrying, and for
//! how long to wait before the next attempt.
//!
//! ⚠️ **This computes a decision; it does not retry anything.** Actually
//! waiting means touching a clock or a timer, and `oqueue-core` does neither
//! (NFR-51, the sans-io rule) — the caller that drives an attempt loop is
//! the one with a runtime to sleep on. ⚠️ **No such caller exists yet**
//! (`M2.10`, closing `M1.56`): the backends classify errors and return
//! them, `object_store`'s own vendor retry covers request-level blips
//! underneath, and this policy's first real caller is `M3`'s composition —
//! an earlier version of this sentence named `M1.15`/`M1.17` as the
//! callers, which no code ever made true. This module answers "retry, and after how long?" as a pure
//! function of the error and the attempt count; it is never the thing that
//! makes the second call.

use crate::Error;
use std::num::NonZeroU32;
use std::time::Duration;

/// Whether an [`Error`] is worth retrying at all, and how patiently.
///
/// `error-handling.md` rule 7's shape: every error a caller might see
/// answers one question mechanically — is retrying this ever worth it?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// Retry indefinitely, with backoff. The backend is asking for
    /// patience, not reporting a failure that will not resolve —
    /// [`Error::SlowDown`], [`Error::Throttled`].
    Forever,
    /// Retry a bounded number of times, then give up. The failure might be a
    /// blip or might be real, and nothing here can tell which —
    /// [`Error::Transient`].
    Bounded,
    /// Never retry automatically. Retrying would not help, or — for
    /// [`Error::PreconditionFailed`] specifically — would actively corrupt
    /// the guarantee the caller asked for.
    Never,
}

impl Error {
    /// This error's [`RetryClass`].
    ///
    /// ⚠️ **[`Error::PreconditionFailed`] is [`RetryClass::Never`], and this
    /// is the one variant on this list that is not merely "retrying would
    /// not help" — retrying it would actively lie.** A lost compare-and-swap
    /// means someone else's write is now the key's current state; retrying
    /// the same conditional write either fails again honestly (the state
    /// moved further) or, if retried with a *fresh* precondition instead of
    /// the original one, silently converts the caller's conditional write
    /// into last-writer-wins — exactly the race the caller reached for a
    /// precondition to avoid (`error-handling.md` rule 9). The only correct
    /// response to a losing CAS is to read the current state and decide
    /// again, which is a new call this crate did not make, not a retry of
    /// the one that lost.
    #[must_use]
    pub const fn retry_class(&self) -> RetryClass {
        match self {
            Self::SlowDown | Self::Throttled => RetryClass::Forever,
            Self::Transient => RetryClass::Bounded,
            Self::ObjectNotFound { .. }
            | Self::EmptyTopicId
            | Self::NegativePartitionId { .. }
            | Self::NegativeOffset { .. }
            | Self::NegativeOffsetDelta { .. }
            | Self::OffsetOverflow { .. }
            | Self::CommitVersionOverflow { .. }
            | Self::NonMonotonicCommitVersion { .. }
            | Self::NegativeTimestamp { .. }
            | Self::NegativeClockAdvance { .. }
            | Self::TimestampOverflow { .. }
            | Self::SecretRejected { .. }
            | Self::EmptyKeyId
            | Self::EncryptionDisabled
            | Self::EmptyObjectKey
            | Self::EmptyByteRange
            | Self::ByteRangeOutOfBounds { .. }
            | Self::ChunkLengthTooLarge { .. }
            | Self::PartTooSmall { .. }
            | Self::PartTooLarge { .. }
            | Self::TooManyParts { .. }
            | Self::ObjectTooLarge { .. }
            | Self::PreconditionFailed { .. }
            | Self::Permanent => RetryClass::Never,
        }
    }
}

/// Decides how long to wait before the next attempt, or that there should
/// not be one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Wait this long, then try again.
    Retry(Duration),
    /// Do not try again.
    GiveUp,
}

/// Exponential backoff, capped, applied per [`RetryClass`].
///
/// ⚠️ **No jitter.** `object_store`'s own `RetryConfig` (ADR-0008) adds
/// decorrelated jitter, and whatever wires this policy up (`M3` — see the
/// module doc) may prefer that or its own shape entirely — this type is the seam-level decision
/// (retry or not, roughly how long), not a claim that this exact backoff
/// shape is what every backend must use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_bounded_attempts: NonZeroU32,
    base_delay: Duration,
    max_delay: Duration,
}

impl RetryPolicy {
    /// A policy that retries [`RetryClass::Bounded`] errors up to
    /// `max_bounded_attempts` times (the first attempt plus this many
    /// retries), with delay starting at `base_delay`, doubling per attempt,
    /// capped at `max_delay`.
    #[must_use]
    pub const fn new(
        max_bounded_attempts: NonZeroU32,
        base_delay: Duration,
        max_delay: Duration,
    ) -> Self {
        Self {
            max_bounded_attempts,
            base_delay,
            max_delay,
        }
    }

    /// Decides whether to retry, given the error the most recent attempt
    /// failed with and how many attempts have been made so far (the failed
    /// one included; the first attempt is `1`).
    #[must_use]
    pub fn decide(&self, error: &Error, attempts_so_far: u32) -> RetryDecision {
        match error.retry_class() {
            RetryClass::Never => RetryDecision::GiveUp,
            RetryClass::Forever => RetryDecision::Retry(self.backoff(attempts_so_far)),
            RetryClass::Bounded => {
                if attempts_so_far >= self.max_bounded_attempts.get() {
                    RetryDecision::GiveUp
                } else {
                    RetryDecision::Retry(self.backoff(attempts_so_far))
                }
            }
        }
    }

    /// `base_delay * 2^(attempts_so_far - 1)`, capped at `max_delay`.
    fn backoff(&self, attempts_so_far: u32) -> Duration {
        let exponent = attempts_so_far.saturating_sub(1);
        // `checked_mul` on the multiplier, not on the `Duration` — an
        // overflowed multiplier and a `Duration` that would itself overflow
        // both mean "more delay than we will ever actually wait", and the
        // cap handles both by saturating to `max_delay`.
        let multiplier = 2u32.checked_pow(exponent).unwrap_or(u32::MAX);
        self.base_delay
            .checked_mul(multiplier)
            .map_or(self.max_delay, |delay| delay.min(self.max_delay))
    }
}
