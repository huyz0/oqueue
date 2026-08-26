//! The crate's error type.
//!
//! `error-handling.md` rule 3 puts one error enum per crate and builds it with
//! `thiserror`; rule 5 says it classifies rather than wraps. ⚠️ `M1` brought
//! a storage backend's classes — `ObjectNotFound`, `PreconditionFailed`,
//! `SlowDown`, `Throttled`, `Transient`, `Permanent` — landing one at a time,
//! each beside the first code able to actually produce it
//! (`docs/internal/product/backlog.md`'s `M1.4` row records where each one
//! moved and why). **All six exist now**: `Permanent` was the last, in
//! `M1.15`. Whose enum they live in was the open question `M0`'s boundary
//! review raised against `contracts.md` rule 17 — ADR-0009 settled it: this
//! one, not `oqueue-store`'s.

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
/// ⚠️ **`Clone`, added in `M1.19`.** `M0.6` added `SecretRejected` carrying key
/// material and dropped `Clone` so no clone could leave another plaintext copy
/// in freed heap (`security.md` rules 8-9); `M0.11` removed the material, and
/// this crate's own note on the derive said adding it back "should require
/// someone to want it" rather than restoring it speculatively. `M1.19`'s
/// `MergingObjectStore::get_many` is that want: one backend fetch answers
/// several of a caller's originally-requested ranges, and a failed fetch must
/// hand each of them its own independent `Err` — `Result<T, E>` has no
/// built-in way to fan one `E` out to several owners without `Clone`. Every
/// variant is a plain value type (identifiers, counts, sizes) with nothing
/// secret left to duplicate, so this is safe now for the same reason it would
/// have been safe the day `M0.11` removed the material.
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

    /// A metadata-log append was not strictly increasing.
    ///
    /// ⚠️ The log is the serialization point `ADR-0020` puts offset
    /// assignment on, so an out-of-order entry is not a late arrival to be
    /// sorted — it is a second writer, or an allocator that lost its place.
    /// Either way the fold that derives offsets would be wrong, so the append
    /// is refused and the log left unchanged (`M3.md` task 14).
    #[error("commit version {got} does not follow {expected_above}")]
    NonMonotonicCommitVersion {
        /// The highest version already accounted for.
        expected_above: u64,
        /// The version offered next.
        got: u64,
    },

    /// Commit-version arithmetic would have exceeded the `u64` range.
    ///
    /// ⚠️ Sibling of [`Error::OffsetOverflow`] and there for the same reason:
    /// the alternative to this error is a version *smaller* than the one it
    /// advanced from, which breaks the ordering `ADR-0020` makes every
    /// staleness comparison rest on, without failing anything.
    #[error("commit version {base} + {delta} overflows u64")]
    CommitVersionOverflow {
        /// The version being advanced.
        base: u64,
        /// How far it was asked to advance.
        delta: u64,
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

    /// A bundled object's footer names an algorithm this build does not know.
    ///
    /// ⚠️ **An error, never a default.** Doc 10 #40 puts the algorithm in the
    /// region header from the format's first commit precisely so a reader can
    /// tell. Treating an unknown value as "stored as written" would hand a
    /// decoder ciphertext and let it decode whatever that happened to look
    /// like — the migration hazard the field exists to prevent, arriving
    /// through the field.
    #[error("region algorithm {code} is not one this build knows")]
    UnknownRegionAlg {
        /// The value read from the footer.
        code: u8,
    },

    /// A bundled object's footer names a format version this build does not
    /// know.
    #[error("bundle format version {version} is not one this build knows")]
    UnknownBundleFormat {
        /// The version read from the trailer.
        version: u8,
    },

    /// A bundled object's footer could not be read.
    ///
    /// ⚠️ Truncated, torn, or declaring lengths that do not fit. These are
    /// bytes an object store returned, so this is a thing that happens rather
    /// than a thing that would mean a bug — `security.md` rule 3 is why the
    /// parser returns this instead of indexing past the end.
    #[error("the bundle footer is malformed at byte {at}")]
    MalformedBundleFooter {
        /// How far in the parser got.
        at: usize,
    },

    /// A region was added with no records in it.
    ///
    /// ⚠️ Bytes that advance no offsets are bytes nothing can ever read: the
    /// fold turns a span's count into offsets, so a region carrying records
    /// under a count of zero is durable, indexed, billed and unreachable.
    #[error("a region must carry at least one record")]
    EmptyRegion,

    /// A region reached the footer writer without a bounded byte range.
    ///
    /// ⚠️ [`ByteRange::Full`](crate::ByteRange::Full) cannot be encoded: an
    /// object holding N regions has none that is the whole of it, and a footer
    /// claiming otherwise would have a reader decode another topic's records as
    /// its own.
    #[error("a bundled region must name a bounded byte range")]
    UnboundedRegion,

    /// A bundle's footer will not fit its own length fields.
    ///
    /// ⚠️ Unreachable in practice — the counts are `u32` — and an error rather
    /// than a truncated length, which would describe fewer regions than the
    /// object holds and have a reader take another topic's bytes as its own.
    #[error("the bundle is too large for its footer to describe")]
    BundleTooLarge,

    /// A writer identity contains a `/`.
    ///
    /// ⚠️ It would let two distinct identities produce one object key by
    /// moving the boundary between the writer and its sequence — and a shared
    /// key means one flush overwriting another's acknowledged records.
    #[error("a writer identity must not contain '/'")]
    MalformedWriterId,

    /// One writer has flushed `u64::MAX` objects.
    ///
    /// ⚠️ An error rather than a wrap, for the reason offset arithmetic
    /// refuses to wrap: the name after a wrap is one already used.
    #[error("this writer's bundle sequence is exhausted")]
    BundleSequenceExhausted,

    /// A bundle was sealed, or read back, with no regions in it.
    ///
    /// ⚠️ An empty object costs what a full one costs, and committing no spans
    /// burns a `CommitVersion` on a record saying nothing happened.
    #[error("a bundled object must hold at least one region")]
    EmptyBundle,

    /// An object does not hold what the index said it holds.
    ///
    /// ⚠️ **Not a client's error and not a transient one.** The index named a
    /// batch, the object came back, and the two disagree — no region for that
    /// partition, two of them, or bytes too short to be a record batch. Every
    /// one of those means the index and object storage have diverged, which a
    /// retry cannot mend and a client cannot fix. It is the loud failure doc 12
    /// §4.6 asks for in place of the silent one: answering an empty partition
    /// here is how a successful poll returns no records.
    #[error("the index named a batch this object does not hold")]
    IndexObjectMismatch,

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

    /// A chunk-addressed read named a length larger than the store's own
    /// chunk size.
    ///
    /// ⚠️ **Not [`Error::ByteRangeOutOfBounds`].** That variant means the
    /// object turned out to be smaller than the range asked of it — a fact
    /// about stored data, knowable only once the store has been consulted.
    /// This one means the caller asked a chunk-addressed store for something
    /// that is not one of its chunks — a fact about the call, knowable before
    /// any request is sent. Only an object's *last* chunk is legitimately
    /// shorter than `chunk_size`; nothing is ever longer.
    #[error("chunk length {length} exceeds the store's chunk size {chunk_size}")]
    ChunkLengthTooLarge {
        /// The rejected length.
        length: u64,
        /// The chunk size the store was built with.
        chunk_size: u64,
    },

    /// The backend is asking the caller to slow down.
    ///
    /// Retry forever with backoff (`error-handling.md` rule 7) — this class
    /// never becomes permanent on its own.
    #[error("backend requested slow-down")]
    SlowDown,

    /// The backend rejected the request as over its rate limit.
    ///
    /// Retry forever with backoff, same as [`Error::SlowDown`].
    #[error("backend throttled the request")]
    Throttled,

    /// A transient backend failure — a network blip, a `500`.
    ///
    /// Retry a bounded number of times, then give up (`error-handling.md`
    /// rule 7) — unlike [`Error::SlowDown`]/[`Error::Throttled`], repeated
    /// occurrence is a real failure rather than expected backpressure.
    #[error("transient backend failure")]
    Transient,

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

    /// A multipart part fell under [`crate::MultipartLimits::min_part_size`],
    /// and was not the upload's last part.
    #[error("part of {bytes} bytes is under the minimum part size of {min}")]
    PartTooSmall {
        /// The rejected part's size.
        bytes: u64,
        /// The minimum a non-last part must meet.
        min: u64,
    },

    /// A multipart part exceeded [`crate::MultipartLimits::max_part_size`].
    #[error("part of {bytes} bytes exceeds the maximum part size of {max}")]
    PartTooLarge {
        /// The rejected part's size.
        bytes: u64,
        /// The maximum any part may have.
        max: u64,
    },

    /// Adding another part would exceed [`crate::MultipartLimits::max_parts`].
    #[error("upload would exceed the maximum of {max} parts")]
    TooManyParts {
        /// The maximum part count.
        max: u32,
    },

    /// Adding a part would exceed [`crate::MultipartLimits::max_object_size`].
    #[error("upload would exceed the maximum object size of {max} bytes")]
    ObjectTooLarge {
        /// The maximum total object size.
        max: u64,
    },

    /// A backend failure that will not succeed on retry: bad credentials, a
    /// malformed request, an unsupported operation, a config that does not
    /// describe a usable endpoint.
    ///
    /// ⚠️ **`M1.4`'s last dissolved variant, landing where it first has a
    /// producer** (`M1.15`, `backlog.md`) — `error-handling.md` rule 6.
    /// Unlike [`Error::SlowDown`]/[`Error::Throttled`]/[`Error::Transient`],
    /// nothing about this ever resolves itself; retrying is never worth it,
    /// same [`crate::RetryClass::Never`] as [`Error::PreconditionFailed`],
    /// but for the opposite reason — this is not a lost race to respect, it
    /// is a request that was never going to succeed.
    #[error("backend failure will not succeed on retry")]
    Permanent,
}

/// The crate's result alias.
pub type Result<T> = core::result::Result<T, Error>;
