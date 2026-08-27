//! Whether a cache may answer this read, and the ways the answer is no.

use crate::{CommitVersion, CoordinatorEpoch, ReadMode};

/// How long an agent may keep serving from a cache it has heard nothing about.
///
/// `ADR-0021`, and doc 12 §4.6's hazard H4: *"If `now - last_delta_received >
/// staleness_limit`, the agent stops serving from cache and either errors out
/// or forces a coordinator round-trip. This converts unbounded staleness into
/// a hard bound you can put in the inequality above."*
///
/// ⚠️ **A version compare cannot do this job**, which is why this is a gate of
/// its own inside [`CacheState::admits`] rather than a freshness clause. A
/// cache that is current and a cache whose push stream died look identical by
/// version: both hold the last version they were told about. Only elapsed time
/// separates them.
///
/// ⚠️ **5 s is sized against the abnormal cases** — a GC pause, a network
/// partition, a coordinator restart — and `ADR-0021` records a *healthy-path*
/// propagation of p50 41 µs measured at `M3.9`. That measurement is not an
/// argument for shrinking this, and the ADR says so at length: it bounds none
/// of the three cases the limit exists for.
///
/// ⚠️ It is also the **floor** for `M5`'s `deletion_delay`, per `ADR-0021`'s
/// inequality. Lowering it there is a different decision from lowering it here.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2 —
/// and it is spelled with its unit so `check-drift.sh`'s `THRESHOLD_RE`
/// actually matches it, which the canonical `max_metadata_staleness` does not.
pub const MAX_METADATA_STALENESS_MS: u64 = 5_000;

/// A position, and the coordinator incarnation that issued it.
///
/// ⚠️ **`ADR-0023`: a version never travels without its epoch.** Two epochs'
/// version lines are independent counters, so a version from one compared
/// against the other is not stale or fresh — it is **meaningless**, and the
/// reading that feels safe is the safe half of a rule whose other half answers
/// a reader with data that does not contain its own write. A rebalance and a
/// failover both bump the epoch, and both are answered the same way: one round
/// trip.
///
/// ⚠️ **It does not yet carry a shard identity, and `ADR-0020` point 1 asks
/// for both.** `MetadataShardId` is `M7`'s and does not exist, so there is one
/// implicit shard and nothing here can enforce the other half. `M7` receives
/// it — `roadmap.md`'s deferral table, and `M7.md`'s own plan — and it is
/// load-bearing there rather than tidy: two shards' coordinators each start at
/// [`CoordinatorEpoch::ZERO`], so a watermark from one would compare *equal by
/// epoch* against the other's cache and be admitted on a version line that
/// never saw the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionWatermark {
    epoch: CoordinatorEpoch,
    version: CommitVersion,
}

impl SessionWatermark {
    /// Pairs a version with the incarnation that issued it.
    #[must_use]
    pub const fn new(epoch: CoordinatorEpoch, version: CommitVersion) -> Self {
        Self { epoch, version }
    }

    /// Which incarnation issued it.
    #[must_use]
    pub const fn epoch(self) -> CoordinatorEpoch {
        self.epoch
    }

    /// The position itself.
    ///
    /// ⚠️ Only comparable against a version from [`epoch`](Self::epoch).
    #[must_use]
    pub const fn version(self) -> CommitVersion {
        self.version
    }
}

/// Why a cache was not allowed to answer.
///
/// ⚠️ **Several reasons, not one**, and collapsing them would be the mistake.
/// They are found by different checks, they say different things to an
/// operator, and — most of all — they have **different remediations**. Some
/// mean the cache's contents are suspect and some mean the cache is merely
/// behind, and a caller that discarded a cache for the second kind would turn
/// `ADR-0023`'s "one round trip per session, paid once" into one full rebuild
/// per session.
///
/// ⚠️ Each carries the numbers that make its message actionable, per
/// `error-handling.md` rule 13. A reason that renders as a bare token is the
/// "log line that says nothing" [`CacheState::admits`] exists to avoid, and an
/// operator chasing a client's impossible lag cannot act on one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum RefreshReason {
    /// The caller asked for an authoritative answer.
    ///
    /// Hazard H1. [`ReadMode::Linearizable`] is never satisfiable from a
    /// cache at any freshness, because the question is about the *end of the
    /// log* rather than about a record in it — a `ListOffsets` answered from
    /// a cache behind the truth reports a log-end-offset below the real one,
    /// which surfaces to a client as **negative consumer lag**.
    ///
    /// ⚠️ **Not a fault, and an alarm must not treat it as one.** It is the
    /// ordinary answer for every `ListOffsets`, so its rate is the
    /// `ListOffsets` rate.
    #[error("the read asked for an authoritative answer, which no cache may give")]
    Authoritative,

    /// The cache has not reached the version the caller wrote.
    ///
    /// Hazard H2: read-your-writes. The caller holds a [`SessionWatermark`]
    /// from its own produce and the cache is behind it. ⚠️ The cache is
    /// **fine** — it is behind, not wrong, and catching it up is the answer.
    #[error("the cache has folded to {applied:?}, short of the required version {wanted}")]
    NotYetVisible {
        /// The version the caller wrote and needs to see.
        wanted: u64,
        /// How far the cache has folded, or `None` if it has folded nothing.
        applied: Option<u64>,
    },

    /// Nothing has arrived for longer than [`MAX_METADATA_STALENESS_MS`].
    ///
    /// Hazard H4. ⚠️ **This one fires whatever the version says**, including
    /// for [`ReadMode::Stale`]: a cache that is current and one whose push
    /// stream died are indistinguishable by version. The cache is not known
    /// to be wrong — it is unverifiable, which is a different thing and is
    /// answered by one round trip rather than by discarding it.
    #[error("nothing has arrived for {silent_for_ms} ms, past the {limit_ms} ms staleness limit")]
    PushStreamSilent {
        /// How long since the last delta.
        silent_for_ms: u64,
        /// The limit it passed — [`MAX_METADATA_STALENESS_MS`].
        limit_ms: u64,
    },

    /// The cache holds state from a coordinator incarnation that is gone.
    ///
    /// Hazard H5. ⚠️ **Not a freshness problem, and the only reason here that
    /// means the contents are wrong.** A failover may have rewound the log, so
    /// the cache's entries are positions on a line that no longer exists — it
    /// can be arbitrarily far *ahead* by version and must still be
    /// **discarded** rather than caught up.
    #[error("the cache belongs to coordinator epoch {cache}, which is not the current {current}")]
    CacheFromAnotherEpoch {
        /// The epoch the cache was built under.
        cache: u64,
        /// The epoch answering now.
        current: u64,
    },

    /// The caller's watermark was issued by a **newer** incarnation than the
    /// one this cache belongs to.
    ///
    /// `ADR-0023`. ⚠️ **The evidence runs the other way here, and that is the
    /// whole variant.** A watermark carries the epoch of the coordinator that
    /// issued it, so a client holding one from an epoch *above* this cache's
    /// has been served by a coordinator this cache has not heard from — a
    /// failover happened and this side has not noticed. ⚠️ **The cache is
    /// wrong, not merely unverifiable, so it is *discarded***: this agent
    /// belongs to a superseded incarnation, and its entries are positions on a
    /// line the failover may have rewound — the same remediation
    /// [`CacheFromAnotherEpoch`](Self::CacheFromAnotherEpoch) gets, for the
    /// same reason. Catching it up instead would keep serving those entries.
    /// ⚠️ **Not the round trip [`WatermarkFromAnotherEpoch`](Self::WatermarkFromAnotherEpoch)
    /// asks for**, which is this enum's whole organizing rule: that one means
    /// the cache is fine and the *watermark* cannot be compared to it.
    ///
    /// ⚠️ **Why it is not [`CacheFromAnotherEpoch`](Self::CacheFromAnotherEpoch)**:
    /// that one fires when this agent's own `current_epoch` disagrees with the
    /// cache, which is a fact this side already knows. This fires when the
    /// only party who knows is the *client*, and the watermark it presents is
    /// the message.
    #[error(
        "the watermark was issued by coordinator epoch {watermark}, \
         above the epoch {cache} this cache belongs to"
    )]
    CacheFromAnOlderEpoch {
        /// The epoch that issued the caller's watermark.
        watermark: u64,
        /// The epoch this cache was built under.
        cache: u64,
    },

    /// The caller's watermark was issued by an **older** incarnation.
    ///
    /// `ADR-0023`. ⚠️ **The cache is current and correct**; it is the
    /// *watermark* that cannot be compared against it, because two epochs'
    /// version lines are independent counters. Discarding the cache here
    /// would be a rebuild for every live session at once, when one round trip
    /// per session is the whole cost — see this enum's own note.
    #[error(
        "the watermark was issued by coordinator epoch {watermark}, \
         not the epoch {cache} this cache was built under"
    )]
    WatermarkFromAnotherEpoch {
        /// The epoch that issued the caller's watermark.
        watermark: u64,
        /// The epoch the cache was built under.
        cache: u64,
    },
}

/// What a cache is being asked, and what it currently holds.
///
/// ⚠️ **Sans-I/O, and a value rather than a method on the cache.** The decision
/// is the caller's — `ReadMode` exists so the *call site* that knows what
/// correctness it needs makes the choice, rather than a cache deciding it is
/// probably fresh enough. Two of the five bugs an independent audit found in a
/// peer system were stale-cache problems producing silent wrongness, and both
/// are that decision made in the wrong place.
#[derive(Debug, Clone, Copy)]
pub struct CacheState {
    epoch: CoordinatorEpoch,
    applied: Option<CommitVersion>,
    silent_for_ms: u64,
}

impl CacheState {
    /// Describes a cache: whose it is, how far it has folded, and how long
    /// since it last heard anything.
    #[must_use]
    pub const fn new(
        epoch: CoordinatorEpoch,
        applied: Option<CommitVersion>,
        silent_for_ms: u64,
    ) -> Self {
        Self {
            epoch,
            applied,
            silent_for_ms,
        }
    }

    /// Whether this cache may answer `mode`, and why not if it may not.
    ///
    /// ⚠️ **The order of the checks is the design**, and it is three tiers.
    /// The epoch fence comes first, for **every** mode, because it is the only
    /// one that says the *contents* are wrong rather than old — a cache from a
    /// rewound line can be arbitrarily far ahead by version and must still not
    /// answer, and a cache from a departed epoch means the *agent* is not the
    /// incarnation it believes it is. [`Linearizable`](ReadMode::Linearizable)
    /// is answered next, because no cache may serve one and every remaining
    /// question is about how good a cache is. The silence breaker comes third
    /// and applies to every mode that does read the cache,
    /// [`Stale`](ReadMode::Stale) included, because no version comparison can
    /// tell a current cache from one whose push stream died. Only then is the
    /// version question asked at all.
    ///
    /// ⚠️ **`Ok(())` is the only way to serve from cache**, and every other
    /// path names a reason. There is deliberately no boolean: a `false` at a
    /// call site becomes a log line that says nothing, and an operator
    /// chasing a client's impossible lag needs to know *which* thing happened
    /// — some are answered by a round trip and some by discarding the cache,
    /// and each variant's own doc says which. ⚠️ **The count is deliberately
    /// not written here**: it was written in four places and went stale in all
    /// four the day a sixth reason was added (`M3.30`).
    ///
    /// # Errors
    ///
    /// The [`RefreshReason`] that forced a coordinator round trip.
    pub fn admits(
        self,
        mode: ReadMode,
        current_epoch: CoordinatorEpoch,
    ) -> Result<(), RefreshReason> {
        // H5, and it comes first for **every** mode, `Linearizable` included:
        // this cache belongs to an incarnation that is gone. Its entries are
        // positions on a line a failover may have rewound, so it can be
        // arbitrarily far *ahead* by version and must still not answer.
        //
        // ⚠️ **`Linearizable` is fenced here even though no cache may answer
        // it**, and that is not a contradiction: a cache from a departed epoch
        // means *this process* is not the incarnation it believes it is, which
        // is a fact about the agent rather than about the cache.
        // `listoffsets.rs` reads an offset out of its own index on
        // `Authoritative` and refuses on anything else, so short-circuiting
        // above this check would let a follower serve a log-end-offset off a
        // rewound line — hazard H1, arriving through the fence built to stop
        // it. `M3.30` did exactly that for one round of review.
        if self.epoch != current_epoch {
            return Err(RefreshReason::CacheFromAnotherEpoch {
                cache: self.epoch.get(),
                current: current_epoch.get(),
            });
        }
        // ⚠️ **A `Linearizable` read is answered before the *freshness*
        // questions are asked**, and the order is the point. No cache may
        // answer one, so everything below is asking how good a cache is for a
        // read that will not consult it — and the answers are *faults*. A
        // silent agent reported `PushStreamSilent` for every `ListOffsets`:
        // a cache alarm firing at the `ListOffsets` rate, which an operator
        // cannot tell from ordinary traffic. Meanwhile `Authoritative` — the
        // reason that is *not* a fault, and the one they need to see — read
        // zero. ⚠️ **Below the epoch fence, not above it**: that one is about
        // the agent, not about freshness.
        if matches!(mode, ReadMode::Linearizable) {
            return Err(RefreshReason::Authoritative);
        }
        // H4's breaker, and it is in front of every mode that reads the cache,
        // `Stale` included: a cache that is current and one whose push stream
        // died hold the same version, and only elapsed silence separates them.
        if self.silent_for_ms > MAX_METADATA_STALENESS_MS {
            return Err(RefreshReason::PushStreamSilent {
                silent_for_ms: self.silent_for_ms,
                limit_ms: MAX_METADATA_STALENESS_MS,
            });
        }
        match mode {
            // Answered above, before anything about the cache was asked.
            ReadMode::Linearizable => Err(RefreshReason::Authoritative),
            ReadMode::Stale => Ok(()),
            // `ADR-0023`: a version compare happens only between versions from
            // one line. A watermark carried across a rebalance is
            // incomparable, not behind.
            // ⚠️ **A *newer* epoch is the one piece of evidence a reader-side
            // cache has that its own coordinator is the departed one**, and
            // answering it as "the cache is current and correct" is exactly
            // backwards. `CoordinatorEpoch` is `Ord`, so the direction is a
            // comparison rather than an inequality.
            ReadMode::AtLeast(want) if want.epoch() > self.epoch => {
                Err(RefreshReason::CacheFromAnOlderEpoch {
                    watermark: want.epoch().get(),
                    cache: self.epoch.get(),
                })
            }
            ReadMode::AtLeast(want) if want.epoch() != self.epoch => {
                Err(RefreshReason::WatermarkFromAnotherEpoch {
                    watermark: want.epoch().get(),
                    cache: self.epoch.get(),
                })
            }
            ReadMode::AtLeast(want) => {
                if self
                    .applied
                    .is_some_and(|applied| applied.get() >= want.version().get())
                {
                    Ok(())
                } else {
                    Err(RefreshReason::NotYetVisible {
                        wanted: want.version().get(),
                        applied: self.applied.map(CommitVersion::get),
                    })
                }
            }
        }
    }
}
