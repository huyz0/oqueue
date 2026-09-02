//! How long a follower's own `SyncGroup` may wait for the leader's.
//!
//! ⚠️ **Its own module for `join_group::deadline`'s own reason**: a ceiling
//! this broker picks unilaterally is a decision, not a helper.

/// The longest a follower's own `SyncGroup` waits for the leader's, whatever
/// that leader is doing.
///
/// ⚠️ **Not derived from any client-supplied value** — `sync_group.rs`'s
/// own module doc: `SyncGroupRequest` carries no timeout field, unlike
/// `JoinGroup`'s `rebalance_timeout_ms`, so this is a fixed ceiling rather
/// than the group's own configured rebalance timeout. Chosen equal to
/// `join_group::deadline::MAX_BARRIER_MS` — the same order of magnitude a
/// real client's own `rebalance.timeout.ms` already bounds the whole
/// join-then-sync round trip to, so this alone is never the tighter limit.
pub(crate) const MAX_SYNC_WAIT_MS: u64 = 3_000_000;
