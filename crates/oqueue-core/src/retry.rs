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
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive match over `Error`'s variants, which \
                  `code-structure.md` rule 17 names as the legitimate case and \
                  `ADR-0040` decides the enum stays flat for: the arms are a \
                  list, not structure, and splitting the function would mean \
                  a `_ =>` arm somewhere, which rule 16 bans precisely because \
                  it absorbs a new variant silently"
    )]
    pub const fn retry_class(&self) -> RetryClass {
        match self {
            // ⚠️ `IndexQuotaExceeded` sits with the throttles rather than with
            // the malformed inputs: the fold was well formed and the index was
            // full, and what relieves it is the coordinator publishing a
            // manifest — which happens on its own, on the sweep interval
            // `ADR-0043` decision 1 makes the real lever. A bounded ladder
            // would give up while the condition was still clearing.
            Self::SlowDown | Self::Throttled | Self::IndexQuotaExceeded { .. } => {
                RetryClass::Forever
            }
            Self::Transient => RetryClass::Bounded,
            Self::ObjectNotFound { .. }
            | Self::EmptyTopicId
            | Self::EmptyPrincipal
            | Self::EmptyGroupId
            | Self::EmptyMemberId
            | Self::IllegalGroupTransition { .. }
            | Self::NegativePartitionId { .. }
            | Self::NegativeOffset { .. }
            | Self::NegativeOffsetDelta { .. }
            | Self::NegativeProducerId { .. }
            | Self::NegativeProducerEpoch { .. }
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
            // Never: a round whose plans overlap, or whose total exceeds what
            // one object may hold, is a round the planner built wrong. The same
            // round retried is the same round.
            | Self::OverlappingCompactionPlans
            | Self::CompactionRoundTooLarge { .. }
            // Never: a malformed or unknown-format object does not become
            // well-formed by being read again.
            | Self::UnknownRegionAlg { .. }
            | Self::UnknownBundleFormat { .. }
            | Self::MalformedBundleFooter { .. }
            // Never, for the same reason one format over: a manifest that is
            // torn, or that is not a manifest at all, reads identically every
            // time. ⚠️ `NotACompositeManifest` in particular means the caller
            // asked the wrong object for the wrong thing, which no wait fixes.
            | Self::MalformedCompositeManifest { .. }
            | Self::NotACompositeManifest
            | Self::UnknownCompositeVersion { .. }
            | Self::ManifestDoesNotMeetHistory { .. }
            | Self::MalformedPartitionManifest { .. }
            | Self::NotAPartitionManifest
            | Self::UnknownPartitionManifestVersion { .. }
            // Never, and `BundleTailTooShort`'s own docs say why it is here
            // rather than in `Bounded`: the object is healthy and the read was
            // too narrow, so the *same* call fails identically forever. A
            // ladder that retried it would loop on an identical GET; what the
            // caller must do is issue a wider one, which is a new call.
            | Self::BundleTailTooShort { .. }
            | Self::EmptyBundle
            | Self::IndexObjectMismatch
            | Self::EmptyRegion
            // Never, and for the same reason the two above are: a metadata log
            // that holds a malformed record holds it identically on every
            // replay. ⚠️ These are the fold's own vocabulary for two
            // malformations the variants above state in the writer's and the
            // reader's, so they classify where those do.
            | Self::EmptySpanInLog { .. }
            | Self::SwapRefused { .. }
            | Self::TrimPastEnd { .. }
            // Never: the records below the start are gone, and asking again
            // does not bring them back. The client resets its own position.
            | Self::BelowLogStart { .. }
            // Never: a quota built with its alarm at or above its ceiling is a
            // configuration, and the same configuration is rejected the same
            // way every time.
            | Self::InvalidIndexQuota { .. }
            | Self::UnboundedRegion
            | Self::BundleTooLarge
            | Self::MalformedWriterId
            | Self::BundleSequenceExhausted
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
    /// The policy a backend uses when its caller names none.
    ///
    /// ⚠️ **UNDERIVED**, and `M3.13` is where it stops being absent rather than
    /// where it becomes measured. `ADR-0008`'s premise — that a real caller
    /// configures the vendor retry "from `RetryPolicy`'s constants" — had no
    /// constants to read, which is why the premise sat unwired from `M1.56`
    /// through `M2.10`. These three numbers are a starting point with the right
    /// shape: three bounded *attempts* — so two retries — doubling from 100 ms,
    /// capped at 5 s. `M14`
    /// measures what they should be against a real backend's error
    /// distribution.
    ///
    /// ⚠️ It bounds only [`RetryClass::Bounded`]. `SlowDown` and `Throttled`
    /// retry forever with backoff by `error-handling.md` rule 7, and no number
    /// here changes that.
    pub const DEFAULT: Self = Self::new(
        match NonZeroU32::new(3) {
            Some(n) => n,
            // Unreachable: 3 is not zero.
            None => panic!("3 is non-zero"),
        },
        Duration::from_millis(100),
        Duration::from_secs(5),
    );

    /// How many **attempts** a [`RetryClass::Bounded`] error gets in total.
    ///
    /// ⚠️ **Attempts, not retries**, and the distinction is load-bearing:
    /// [`decide`](Self::decide) gives up once `attempts_so_far` reaches this,
    /// so `1` means one try and no retry. A vendor configuration that counts
    /// retries — `object_store`'s does — needs this minus one, which is what
    /// `oqueue-store`'s translation does and what a straight copy would get
    /// wrong in the direction that retries when told not to.
    #[must_use]
    pub const fn max_bounded_attempts(&self) -> NonZeroU32 {
        self.max_bounded_attempts
    }

    /// The first delay, which doubles per attempt.
    #[must_use]
    pub const fn base_delay(&self) -> Duration {
        self.base_delay
    }

    /// The ceiling that doubling is capped at.
    #[must_use]
    pub const fn max_delay(&self) -> Duration {
        self.max_delay
    }

    /// A policy that gives a [`RetryClass::Bounded`] error
    /// `max_bounded_attempts` **attempts in total** — the first try included,
    /// so `1` means one try and no retry — with delay starting at
    /// `base_delay`, doubling per attempt, capped at `max_delay`.
    ///
    /// ⚠️ The argument is the number [`max_bounded_attempts`] returns and the
    /// number [`decide`] compares `attempts_so_far` against. Reading it as a
    /// retry count is off by one in the direction that retries when told not
    /// to, which is the defect it caused once already in `oqueue-store`'s
    /// vendor translation.
    ///
    /// [`max_bounded_attempts`]: Self::max_bounded_attempts
    /// [`decide`]: Self::decide
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
