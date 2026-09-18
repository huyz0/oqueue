//! A ceiling the fold refuses to cross, and the alarm that fires before it.
//!
//! ⚠️ **Nothing here evicts, and `M3.11`'s reason is why** (`ADR-0043`
//! decision 3). Evicting index entries gives back range a rebuild cannot
//! restore: replaying the log reproduces the same count and sheds the same
//! entries again, so the degraded mode's own recovery is a loop that cannot
//! converge. A ceiling that refuses is a ceiling a replay reaches identically
//! and stops at, which is the difference between a bound and a leak.
//!
//! ⚠️ **No quota by default, and that is not a disabled safety check.**
//! [`IndexState`] is the fold every materialization shares, including the
//! fakes a test builds by hand. A ceiling is a deployment's choice about one
//! coordinator's memory, so inventing one here would put an arbitrary number
//! in the path of every caller with no memory to run out of. The coordinator
//! sets it (`ADR-0043` decision 3).
//!
//! ⚠️ **The ceiling is over the total, not over a tier** (`ADR-0043`
//! decision 1). Above a 40.2-second sweep un-absorbed history dominates and
//! below it the tail does, so a ceiling on either lets the other run — at the
//! intervals that ADR tabulates, one on history alone would let a node reach
//! 23 GB without firing. [`Tiers`](crate::Tiers) is what says *which* tier
//! moved once it has.

use super::IndexState;
use super::projection::{Projected, StagedSpans};
use crate::{Error, Result};

/// The ceiling a fold refuses to cross, and where the alarm sits under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexQuota {
    ceiling: usize,
    alarm_at: usize,
}

impl IndexQuota {
    /// Builds a quota.
    ///
    /// ⚠️ **The alarm is strictly under the ceiling**, because an alarm that
    /// fires *at* the refusal is not an alarm — it is the incident. The whole
    /// value of the degraded mode is the interval between the two, which is
    /// where a sweep interval can still be shortened (`ADR-0043`'s own lever)
    /// before any producer loses an ack.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidIndexQuota`] if the ceiling is zero, or if the alarm is
    /// not below it.
    pub const fn new(ceiling: usize, alarm_at: usize) -> Result<Self> {
        if ceiling == 0 || alarm_at >= ceiling {
            return Err(Error::InvalidIndexQuota { ceiling, alarm_at });
        }
        Ok(Self { ceiling, alarm_at })
    }

    /// The entry count a fold may not take the index past.
    #[must_use]
    pub const fn ceiling(self) -> usize {
        self.ceiling
    }

    /// The entry count at which the index is already in trouble.
    #[must_use]
    pub const fn alarm_at(self) -> usize {
        self.alarm_at
    }

    /// Whether `entries` is at or past the alarm.
    #[must_use]
    pub const fn alarmed(self, entries: usize) -> bool {
        entries >= self.alarm_at
    }

    /// Whether folding to `entries` would cross the ceiling.
    ///
    /// ⚠️ **Strictly past**, so a fold landing exactly on the ceiling is
    /// allowed: the ceiling is a count the index may hold, not one it may
    /// approach. A `>=` here would make the effective bound `ceiling - 1` and
    /// leave every figure derived from it off by one entry.
    #[must_use]
    pub const fn would_cross(self, entries: usize) -> bool {
        entries > self.ceiling
    }
}

/// How close the index is to its ceiling.
///
/// ⚠️ **Observable before the refusal rather than at it**, which is the
/// difference between a degraded mode and an outage: an operator reading
/// [`Alarmed`](Self::Alarmed) has the interval `IndexQuota::new` insists on to
/// shorten the sweep before a producer loses an ack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    /// No quota is set, or the index is under the alarm.
    Nominal,
    /// At or past the alarm, and still folding.
    Alarmed,
}

impl IndexState {
    /// An empty state that refuses to fold past `quota`.
    #[must_use]
    pub fn with_quota(quota: IndexQuota) -> Self {
        Self {
            quota: Some(quota),
            ..Self::default()
        }
    }

    /// The ceiling this state folds under, if any.
    #[must_use]
    pub const fn quota(&self) -> Option<IndexQuota> {
        self.quota
    }

    /// Whether the index is already in trouble.
    ///
    /// ⚠️ **Read after a fold that succeeded**, which is the point: an
    /// operator seeing [`Pressure::Alarmed`] still has every partition's acks
    /// and the interval `IndexQuota::new` insists on to shorten the sweep.
    /// Reading it only when a fold is refused would be reading it at the
    /// incident.
    ///
    /// ⚠️ **One batch can cross both lines at once**, and no arrangement of
    /// this can prevent that: a fold is all-or-nothing, so a batch large
    /// enough to take the index from under the alarm to over the ceiling is
    /// refused having never reported [`Alarmed`](Pressure::Alarmed). The
    /// interval the alarm buys is measured in *batches*, and a deployment
    /// whose batches are a large fraction of its ceiling has no interval to
    /// buy — which is a statement about how the ceiling was chosen, not
    /// something this can report its way out of.
    #[must_use]
    pub const fn pressure(&self) -> Pressure {
        match self.quota {
            Some(quota) if quota.alarmed(self.tiers.total()) => Pressure::Alarmed,
            _ => Pressure::Nominal,
        }
    }

    /// Refuses a batch that would take the index past its ceiling.
    ///
    /// ⚠️ **Before any mutation, like every other refusal here** — guarantee 2,
    /// and it is what makes the degraded mode replayable: the log a refused
    /// fold leaves behind is the log a rebuild reads, so a replay stops in the
    /// same place rather than in a different one.
    ///
    /// ⚠️ **Counted from the same deltas the two loops below apply**, not from
    /// a second description of them — a partition's contribution is its copy's
    /// count against the live one, which is the arithmetic those loops do. A
    /// separate estimate is the shape `M5.13`'s first round found three
    /// defects in.
    pub(super) fn check_quota(
        &self,
        staged: &StagedSpans<'_>,
        projected: &Projected<'_>,
    ) -> Result<()> {
        let Some(quota) = self.quota else {
            return Ok(());
        };

        // ⚠️ Signed, because a batch can shrink the index: a publication
        // absorbs a hundred references into one and a swap retires more than
        // it installs. An unsigned running total would wrap on the way past a
        // ceiling it never reaches.
        let mut contributions: Vec<(&str, i32, i64)> = Vec::new();
        for ((topic, partition), (_, entries)) in staged {
            if projected.contains_key(&(*topic, *partition)) {
                continue;
            }
            let delta = i64::try_from(entries.len()).unwrap_or(i64::MAX);
            contributions.push((topic.as_str(), partition.get(), delta));
        }
        for ((topic, partition), slot) in projected {
            let before = self
                .partition(topic, *partition)
                .map_or(0, |held| held.tiers().total());
            let delta = i64::try_from(slot.tiers().total()).unwrap_or(i64::MAX)
                - i64::try_from(before).unwrap_or(i64::MAX);
            contributions.push((topic.as_str(), partition.get(), delta));
        }

        let net: i64 = contributions.iter().map(|(_, _, delta)| *delta).sum();
        let held = i64::try_from(self.tiers.total()).unwrap_or(i64::MAX);
        let would_be = usize::try_from(held.saturating_add(net).max(0)).unwrap_or(usize::MAX);
        if !quota.would_cross(would_be) {
            return Ok(());
        }

        // ⚠️ **The largest contributor, and ties broken by name.** A batch can
        // touch thousands of partitions and an operator can act on one; the
        // one that grew most is where the growth is. ⚠️ Deterministic on
        // purpose — `staged` and `projected` are hash maps, so a refusal
        // naming whichever came first would name a different partition per run
        // and read as two incidents rather than one.
        let (topic, partition) = contributions
            .into_iter()
            .max_by(|left, right| {
                left.2
                    .cmp(&right.2)
                    .then_with(|| right.0.cmp(left.0))
                    .then_with(|| right.1.cmp(&left.1))
            })
            .map_or((String::new(), 0), |(topic, partition, _)| {
                (topic.to_owned(), partition)
            });
        Err(Error::IndexQuotaExceeded {
            topic,
            partition,
            would_be,
            ceiling: quota.ceiling(),
        })
    }
}
