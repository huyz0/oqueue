//! The crate's error type.
//!
//! `error-handling.md` rule 3 puts one error enum per crate and builds it with
//! `thiserror`; rule 5 says it classifies rather than wraps. ⚠️ This is the
//! *start* of that taxonomy, not the whole of it: the identifiers, redaction
//! and the three seams are all here, and `M1` brings a storage backend's
//! classes — `NotFound`, `PreconditionFailed`, `SlowDown`, `Throttled`,
//! `Transient`, `Permanent`.
//! ⚠️ **Whose enum those land in is an open decision**, not this one by
//! default: M0's boundary review found both seams returning `crate::Error`
//! against `contracts.md` rule 17, and `M1.md` blocks its taxonomy task on an
//! ADR.

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
///
/// ⚠️ **Not `Clone`.** `M0.6` added `SecretRejected` carrying key material and
/// dropped `Clone` so no clone could leave another plaintext copy in freed heap
/// (`security.md` rules 8-9). `M0.11` removed the material, so that reason is
/// spent — but the derive stays off, because nothing needs to clone an error
/// and adding it back should require someone to want it.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
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

    /// A timestamp was negative.
    #[error("timestamp must not be negative, got {got}")]
    NegativeTimestamp {
        /// The rejected value.
        got: i64,
    },

    /// A clock was asked to move backwards.
    ///
    /// ⚠️ Distinct from [`Error::NegativeTimestamp`] for the same reason
    /// [`Error::NegativeOffsetDelta`] is distinct from
    /// [`Error::NegativeOffset`]: a negative timestamp means a peer or a
    /// decoder got it wrong, a negative advance means our own code did.
    #[error("cannot advance a clock by a negative amount, got {got}")]
    NegativeClockAdvance {
        /// The rejected delta.
        got: i64,
    },

    /// A clock advance would have left the protocol's `int64` range.
    #[error("timestamp {base} + {delta} exceeds the maximum timestamp")]
    TimestampOverflow {
        /// The timestamp the advance started from.
        base: i64,
        /// The amount that could not be added.
        delta: i64,
    },

    /// A wrapped key was not produced under the key id it was presented with.
    ///
    /// ⚠️ **Carries the key *identifier*, never the material.** `M0.6` created
    /// this variant holding the rejected bytes inside a `Redacted`, which was
    /// safe from formatting but still read literally against FR-44 and
    /// `security.md` rule 6 — both of which say a secret must not reach an
    /// error variant at all. `M0.11` resolved that by removing the secret
    /// rather than by amending the standard, which is the outcome that needed
    /// no exception. A key id is an identifier: it names material without
    /// being it, and it is the thing an operator needs to act.
    #[error("secret rejected while {context} (key {key_id})")]
    SecretRejected {
        /// What was being attempted, in terms an operator can act on.
        context: &'static str,
        /// Which key it was attempted under.
        ///
        /// ⚠️ **A key id names a tenant, and under BYOK a KMS ARN carries the
        /// customer's account id** — `error-handling.md` rule 14 says default
        /// to less. It is here anyway, on the same trade as
        /// [`Error::ObjectNotFound`]'s key: an identifier is not a capability,
        /// holding one grants nothing without credentials, and "a key was
        /// rejected" with no id is unactionable for the operator who has to
        /// find out which. ⚠️ Like that variant, this depends on not reaching
        /// a client — `error-handling.md` rule 12, and `M2`'s decision.
        key_id: crate::KeyId,
    },

    /// A key id was empty.
    #[error("key id is empty")]
    EmptyKeyId,

    /// The configured key provider does not encrypt.
    ///
    /// ⚠️ Returned by `oqueue-crypto`'s no-op provider rather than silently
    /// succeeding — see ADR-0006.
    #[error("encryption is not enabled on this deployment")]
    EncryptionDisabled,

    /// No object is stored under that key.
    #[error("no object stored under key {key}")]
    ObjectNotFound {
        /// The key that was looked up.
        ///
        /// ⚠️ **An object key is derived from a topic name, so it names a
        /// tenant** — as does `Error::SecretRejected`'s key id, and both are
        /// the same trade rather than opposite ones. Rule 14 says
        /// default to less and name a resource by something that does not
        /// double as a capability. An object key is not a capability — holding
        /// one grants nothing without credentials — while "not found" with no
        /// key is unactionable for the operator who has to find out why.
        ///
        /// ⚠️ **This depends on the variant not reaching a client**, which
        /// nothing yet enforces: `M2` decides what a fetch failure looks like
        /// on the wire, and `error-handling.md` rule 12 says that shape is not
        /// this enum's `Display`.
        key: crate::ObjectKey,
    },

    /// An object key was empty.
    #[error("object key is empty")]
    EmptyObjectKey,

    /// A [`crate::ByteRange::bounded`] was constructed with a zero length.
    #[error("byte range must not have a zero length")]
    EmptyByteRange,

    /// A ranged `get` asked for bytes past the object's actual size.
    ///
    /// ⚠️ **Not `ObjectNotFound`.** The object exists; the requested range
    /// does not fit inside it. A caller that conflates the two would retry a
    /// range error as if the object might reappear, which it will not.
    #[error(
        "byte range [{offset}, {offset} + {length}) is out of bounds for object {key} \
         (size {object_size})"
    )]
    ByteRangeOutOfBounds {
        /// The object the range was requested against.
        key: crate::ObjectKey,
        /// The requested range's start.
        offset: u64,
        /// The requested range's length.
        length: u64,
        /// The object's actual size.
        object_size: u64,
    },

    /// A conditional `put` lost the race its [`crate::Precondition`] named.
    ///
    /// ⚠️ **Never retried automatically at the point it is detected**
    /// (`error-handling.md` rule 9) — retrying a lost compare-and-swap
    /// silently converts it into last-writer-wins, which is exactly the
    /// invariant a caller reached for a `Precondition` to get.
    #[error("precondition failed for {key}")]
    PreconditionFailed {
        /// The key the conditional write targeted.
        key: crate::ObjectKey,
    },
}

/// The crate's result alias.
pub type Result<T> = core::result::Result<T, Error>;
