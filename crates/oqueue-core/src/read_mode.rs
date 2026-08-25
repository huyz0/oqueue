//! How fresh a read has to be, and who is allowed to answer it.

use crate::SessionWatermark;

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
    /// Answer only from an index at or past this watermark, otherwise refresh.
    ///
    /// Read-your-writes, hazard H2: a producer carries its Produce response's
    /// commit version as a session watermark, and a consumer in the same
    /// session fetching at that watermark cannot be told the records are not
    /// there yet.
    ///
    /// ⚠️ **A [`SessionWatermark`] rather than a bare version** — `ADR-0023`.
    /// A version from one coordinator incarnation compared against another's
    /// line is not stale or fresh, it is meaningless, and the half of that
    /// mistake which feels safe is the half that answers a reader with data
    /// missing its own write. Carrying the epoch makes the pairing
    /// unforgettable rather than a rule to remember.
    AtLeast(SessionWatermark),
    /// Never answer from cache; route to the authoritative coordinator.
    ///
    /// Hazard H1: `ListOffsets` answered from a cache behind the truth reports
    /// a log-end-offset below the real one, which surfaces to a client as
    /// **negative consumer lag**. There is no cache freshness that makes this
    /// safe, because the answer is a claim about the end of the log rather
    /// than about a record in it.
    Linearizable,
}

// ⚠️ **There is deliberately no `satisfiable_from_cache` here any more.**
// It answered the version question and only that one, and `M3.10` found that
// shipping a *partial* admission test beside a complete one is how the partial
// one gets called: `ADR-0021`'s silence breaker and `ADR-0023`'s epoch fence
// are not clauses a caller may skip, and neither is expressible on this enum
// alone. [`CacheState::admits`](crate::CacheState::admits) is the whole
// answer, and it is the only one.
