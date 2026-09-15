//! How long a follower's own `SyncGroup` may wait for the leader's.
//!
//! ⚠️ **Its own module for `join_group::deadline`'s own reason**: a ceiling
//! this broker picks unilaterally is a decision, not a helper.

use std::time::Duration;

/// How long a follower waits for the leader's submission, from the
/// `rebalance_timeout_ms` the group's own round was opened with.
///
/// ⚠️ **Derived, and `M4.66` is why it had to become so.** `SyncGroupRequest`
/// carries no timeout field, so this cannot be read off the request the way
/// [`super::super::join_group::deadline::barrier_ms`] reads `JoinGroup`'s —
/// it comes from the round instead, recorded by `join_group::round` for each
/// member it admits and held on the group's barrier entry. A non-positive value is a client bug and becomes
/// [`MIN_SYNC_WAIT_MS`] rather than no wait at all; anything above
/// [`MAX_SYNC_WAIT_MS`] becomes that ceiling.
///
/// ⚠️ **`None` is a group whose round this broker never opened** — every
/// test that drives the coordinator directly, and nothing a real client
/// produces, since a follower can only be syncing at a generation some
/// `JoinGroup` round assembled. It falls back to the ceiling, which is the
/// behaviour every caller had before this value existed.
///
/// ⚠️ **The floor is the load-bearing half, and the first version of this
/// had only the ceiling.** `join_group::deadline::barrier_ms` clamps at zero
/// and needs no floor, because a zero there shortens only the *asking*
/// member's own round; routed into this phase the same zero shortens every
/// *other* member's wait, and review measured a follower answered
/// `REBALANCE_IN_PROGRESS` at `waited = 0ns` — handed a rejoin instead of
/// the slice a leader submitted a millisecond later. `heartbeat::deadline`
/// is the sibling that already had this exact floor for this exact reason,
/// and [`MIN_SYNC_WAIT_MS`] is its constant rather than a second opinion.
pub(crate) fn sync_wait_ms(rebalance_timeout: Option<Duration>) -> u64 {
    let Some(timeout) = rebalance_timeout else {
        return MAX_SYNC_WAIT_MS;
    };
    u64::try_from(timeout.as_millis())
        .unwrap_or(MAX_SYNC_WAIT_MS)
        .clamp(MIN_SYNC_WAIT_MS, MAX_SYNC_WAIT_MS)
}

/// The shortest a follower waits, whatever its group asked for.
///
/// ⚠️ **`heartbeat::deadline::MIN_SESSION_TIMEOUT_MS`, not a number of this
/// module's own**, because the question is the same one: how short a
/// client-supplied duration in the group protocol this broker will honour
/// before a healthy peer is treated as gone. A sync phase shorter than the
/// shortest session this broker honours cannot be right, and two constants
/// for one decision is the second opinion nobody made.
pub(crate) const MIN_SYNC_WAIT_MS: u64 = 6_000;

// ⚠️ **The tie is an assertion, not an expression, and `check-drift.sh` is
// why.** Non-negotiable 2 pins a threshold by its literal value, so a constant
// written as `MIN_SESSION_TIMEOUT_MS.unsigned_abs() as u64` reads as `''` to
// the gate and cannot be pinned at all. Writing the literal and asserting the
// sibling still holds it keeps both properties: the gate sees a value to pin,
// and a commit that moves the session floor without deciding about this one
// fails to compile rather than silently separating them.
const _: () = assert!(crate::heartbeat::deadline::MIN_SESSION_TIMEOUT_MS == 6_000);

/// The longest a follower's own `SyncGroup` waits for the leader's, whatever
/// that leader is doing and whatever its group asked for.
///
/// ⚠️ **A ceiling on the group's number, not the number itself**, which is
/// what `M4.66` changed and what the sentence here used to get wrong. It
/// said this was "chosen equal to `join_group::deadline::MAX_BARRIER_MS` —
/// the same order of magnitude a real client's own `rebalance.timeout.ms`
/// already bounds the whole join-then-sync round trip to, so this alone is
/// never the tighter limit". Both halves were false in the direction that
/// matters: either reference client seeds `rebalance_timeout_ms` from
/// `max.poll.interval.ms`, **300 s** by default, so this was ten times
/// *looser* rather than the same order of magnitude — and "never the tighter
/// limit" was therefore true only by being vacuous, since the join half
/// clamps the client's number and this half ignored it.
///
/// ⚠️ **Nothing else bounded the sync phase**, which is what made that
/// sentence load-bearing rather than merely wrong. A parked follower is not
/// heartbeating — both reference clients disable the heartbeat thread for
/// the duration of a rebalance — so `Heartbeats::sweep`, which is
/// request-driven and has no background reaper, is not running on its
/// behalf. A leader that dies between `JoinBarrierComplete` and its own
/// `SyncGroup` therefore left every follower here for fifty minutes:
/// `reopened::a_follower_that_waits_out_the_deadline_is_told_to_rejoin_not_given_a_fatal_code`
/// is that case, and `M4.48` made the code it ends on retriable without
/// changing how long it takes to get there. Real Kafka bounds the same phase
/// the same way, re-arming each member's expiration on the group's
/// `rebalanceTimeoutMs` rather than its `sessionTimeoutMs` once the join
/// completes, and for this reason.
///
/// ⚠️ Equal to [`super::super::join_group::deadline::MAX_BARRIER_MS`] and
/// still deliberately so: both are ceilings on the same client-supplied
/// field, so one number is the honest answer and two would be a second
/// decision nobody made.
pub(crate) const MAX_SYNC_WAIT_MS: u64 = 3_000_000;

#[cfg(test)]
mod tests {
    use super::{MAX_SYNC_WAIT_MS, MIN_SYNC_WAIT_MS, sync_wait_ms};
    use std::time::Duration;

    #[test]
    fn an_unopened_round_falls_back_to_the_ceiling() {
        assert_eq!(sync_wait_ms(None), MAX_SYNC_WAIT_MS);
    }

    #[test]
    fn an_ordinary_group_waits_what_its_round_was_opened_with() {
        assert_eq!(sync_wait_ms(Some(Duration::from_mins(5))), 300_000);
        assert_eq!(sync_wait_ms(Some(Duration::from_secs(45))), 45_000);
    }

    /// ⚠️ **A member asking for nothing must not answer every other
    /// member's follower instantly**, which the first version of this did:
    /// review drove a `JoinGroup` with `rebalance_timeout_ms=0` through the
    /// real handler and measured a different member's follower answered
    /// `REBALANCE_IN_PROGRESS` at `waited = 0ns`.
    #[test]
    fn nothing_a_group_asks_for_falls_below_the_floor() {
        assert_eq!(sync_wait_ms(Some(Duration::ZERO)), MIN_SYNC_WAIT_MS);
        assert_eq!(
            sync_wait_ms(Some(Duration::from_millis(1))),
            MIN_SYNC_WAIT_MS
        );
        assert_eq!(
            sync_wait_ms(Some(Duration::from_millis(MIN_SYNC_WAIT_MS))),
            MIN_SYNC_WAIT_MS
        );
    }

    #[test]
    fn nothing_a_group_asks_for_exceeds_the_ceiling() {
        assert_eq!(
            sync_wait_ms(Some(Duration::from_millis(MAX_SYNC_WAIT_MS + 1))),
            MAX_SYNC_WAIT_MS
        );
        assert_eq!(sync_wait_ms(Some(Duration::MAX)), MAX_SYNC_WAIT_MS);
    }
}
