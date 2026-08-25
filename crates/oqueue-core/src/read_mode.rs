//! How fresh a read has to be, and who is allowed to answer it.

use crate::CommitVersion;

/// The freshness a caller requires of the index it is answered from.
///
/// `M3.md` task 16's three modes, and the routing rule for each. The whole
/// point is that the choice is made by the *caller*, at the call site that
/// knows what correctness it needs, rather than by a cache deciding it is
/// probably fresh enough.
///
/// ⚠️ The hazards this enum exists for are doc 12 §4.6's, which `M3.md`'s
/// Risks section calls the substance of this milestone. Two of the five bugs
/// an independent audit found in a peer system were stale-cache problems
/// producing **silent wrongness** — a poll that succeeds and returns no
/// records, a lag that reads negative. Neither fails anything, which is why
/// the freshness choice is a type at the call site rather than a cache's
/// judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadMode {
    /// Answer from cache at whatever version it holds. The Fetch path: a
    /// consumer streaming the log is already behind by construction, and a
    /// slightly older index costs it nothing but a later batch.
    Stale,
    /// Answer only from an index at or past this version, otherwise refresh.
    ///
    /// Read-your-writes, hazard H2: a producer carries its Produce response's
    /// commit version as a session watermark, and a consumer in the same
    /// session fetching at that watermark cannot be told the records are not
    /// there yet.
    AtLeast(CommitVersion),
    /// Never answer from cache; route to the authoritative coordinator.
    ///
    /// Hazard H1: `ListOffsets` answered from a cache behind the truth reports
    /// a log-end-offset below the real one, which surfaces to a client as
    /// **negative consumer lag**. There is no cache freshness that makes this
    /// safe, because the answer is a claim about the end of the log rather
    /// than about a record in it.
    Linearizable,
}

impl ReadMode {
    /// Whether an index cached at `cached` is **fresh enough by version** for
    /// this read.
    ///
    /// ⚠️ This answers the version question and only that one. It is not the
    /// whole admission decision: `ADR-0021` puts a second, independent gate in
    /// front of the cache — past `max_metadata_staleness` (5 s) since the last
    /// delta, an agent stops serving from cache whatever this returns, because
    /// a version compare cannot tell a cache that is current from one whose
    /// push stream died. `Stale` is subject to that breaker too.
    ///
    /// ⚠️ [`ReadMode::Linearizable`] is `false` for every `cached`, however
    /// fresh. That is not a conservative approximation — it is the mode's
    /// definition, and a version comparison that let it through would be
    /// hazard H1 restored.
    #[must_use]
    pub const fn satisfiable_from_cache(self, cached: CommitVersion) -> bool {
        match self {
            Self::Stale => true,
            Self::AtLeast(want) => cached.get() >= want.get(),
            Self::Linearizable => false,
        }
    }
}
