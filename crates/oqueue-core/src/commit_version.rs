//! The coordinator's single monotonic scalar, and arithmetic that refuses to
//! wrap.

use crate::{Error, Result};

/// A position in one metadata log, stamped by that log's single allocator as
/// each commit lands.
///
/// # Invariant
///
/// **No operation on it ever wraps.** `ADR-0020` makes this the one scalar
/// staleness compares on — this said `ReadMode::AtLeast(v)` was "a `u64`
/// compare and nothing more" until `M3.37`, which `ADR-0023` falsified: an
/// `AtLeast` watermark carries the epoch that issued it, and
/// [`CacheState::admits`](crate::CacheState::admits) settles both epoch
/// directions before any version is compared. The compare *within* an epoch is
/// still exactly a `u64` one, which is why a silent wrap is the only
/// arithmetic result that breaks ordering without failing anything: the successor comes back smaller than
/// what it started from, and every later comparison is wrong. So
/// [`CommitVersion::advance`] returns [`Error::CommitVersionOverflow`] rather
/// than a number, exactly as [`Offset::add`](crate::Offset::add) does.
///
/// ⚠️ **It is a counter, not a timestamp or a hash** (`M3.md` task 1). Nothing
/// downstream may compare *the scalar* as anything but a `u64` — and nothing
/// may compare it alone, because `ADR-0023` pairs it with an epoch precisely
/// so a version from a superseded incarnation cannot read as fresh.
///
/// ⚠️ **It is only ordered within its own metadata log.** `ADR-0020` shards
/// the log and gives each shard its own allocator, so comparing versions
/// across shards is meaningless — a version must never travel without the
/// identity of the shard that issued it. The shard identifier itself is `M7`'s
/// (`MetadataShardId`), so today there is one implicit log and nothing here
/// can enforce the rule; it is stated so that the first code to hold two logs
/// does not have to rediscover it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitVersion(u64);

impl CommitVersion {
    /// The version a log holds before anything has committed to it.
    pub const ZERO: Self = Self(0);

    /// Builds a commit version.
    ///
    /// Total, unlike [`Offset::new`](crate::Offset::new): every `u64` is a
    /// version, because the log starts at zero and only ever counts up.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The version, as the `u64` every comparison reduces to.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Advances by `delta`.
    ///
    /// # Errors
    ///
    /// [`Error::CommitVersionOverflow`] if the sum would leave the `u64`
    /// range. ⚠️ The error is the point: the alternative is a value smaller
    /// than `self`, which is the one outcome the ordering invariant cannot
    /// survive.
    pub const fn advance(self, delta: u64) -> Result<Self> {
        match self.0.checked_add(delta) {
            Some(sum) => Ok(Self(sum)),
            None => Err(Error::CommitVersionOverflow {
                base: self.0,
                delta,
            }),
        }
    }
}

impl core::fmt::Display for CommitVersion {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
