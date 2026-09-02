//! The join barrier: who is mid-round for a group, and when a round closes.
//!
//! ⚠️ **Its own module because this is state [`oqueue_core::GroupCoordinator`]
//! deliberately does not carry.** `M4.2`'s own trait holds
//! state/generation/assignment-epoch — the three values every implementation
//! must agree on, sans-I/O (`ADR-0034`). Who is mid-`JoinGroup`, how many
//! members a round expects, and the deadline it closes by are this broker's
//! own bookkeeping: async, per-node, and never durable, the same split
//! `fetch::park` draws between `IndexWatch` (the coordinator's own) and the
//! parking loop around it (the broker's own).
//!
//! ⚠️ **Members are admitted one at a time, by re-running `M4.6`'s own
//! `elect`.** A member whose addition would leave the round's own running
//! candidate set empty is refused `INCONSISTENT_GROUP_PROTOCOL` and never
//! enrolled — never added to `members`, never counted toward `expected` —
//! rather than being collected and only checked once the round closes. This
//! is what makes the round's own eventual `elect` call at close time
//! infallible: every enrolled member is, by construction, pairwise
//! compatible with every other.
//!
//! ⚠️ **`Dead` is unreachable through this path in this milestone.**
//! [`oqueue_core::GroupEvent::Expire`] has no caller anywhere in this
//! workspace (`group_state.rs`'s own module doc: "no `DeleteGroups`-style
//! admin API exists in this milestone's own scope") — a group this broker's
//! `GroupCoordinator` implementation ever hands out only ever reaches
//! `Dead` from a caller `M12`'s own future scope adds. Reaching it here
//! would mean this bookkeeping and the coordinator's own state have
//! diverged, a bug rather than a case a client can trigger — refused
//! rather than panicked on, `security.md` rule 3's instinct applied to an
//! internal invariant instead of untrusted wire bytes.

use oqueue_core::{Candidate, GenerationId, GroupCoordinator, GroupEvent, GroupId, GroupState};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// Mints a fresh member id — process-unique, [`crate::writer_id::WriterId`]'s
/// own `MINTED`-counter idiom, `member_id.rs`'s own "the broker's own
/// minting scheme never produces an empty one" contract.
static MINTED: AtomicU64 = AtomicU64::new(0);

/// A fresh member id no client chose. Infallible in practice — `"member-"`
/// is a non-empty literal prefix, so the wrapped string can never be empty —
/// but [`oqueue_core::MemberId::new`] still returns a `Result`, and
/// `code-structure.md` rule 26 bans `.unwrap()`/`.expect()` regardless of
/// how certain that is (`init_producer_id.rs`'s own precedent for a
/// provably-unreachable error arm).
pub(crate) fn mint_member_id() -> oqueue_core::MemberId {
    let n = MINTED.fetch_add(1, Ordering::Relaxed);
    oqueue_core::MemberId::new(format!("member-{n}"))
        .unwrap_or_else(|_| unreachable!("\"member-{n}\" always starts with a non-empty literal"))
}

/// `rebalance_timeout_ms` is absent below `JoinGroup` v1 and decodes to `0`
/// (`oqueue_codec::join_group`'s own doc) — `M4.7`'s own decision named
/// there as this task's to make: an absent value falls back to
/// `session_timeout_ms`, matching real Kafka's own v0 behaviour (the
/// rebalance timeout and the session timeout were the same wire field
/// before `KIP-62` split them).
pub(crate) const fn effective_timeout_ms(
    rebalance_timeout_ms: i32,
    session_timeout_ms: i32,
) -> i32 {
    if rebalance_timeout_ms > 0 {
        rebalance_timeout_ms
    } else {
        session_timeout_ms
    }
}

/// One member's own contribution to a round: what a response echoes back
/// for it, and what `elect` needs from it.
#[derive(Debug, Clone)]
pub(crate) struct RoundMember {
    pub(crate) member_id: String,
    pub(crate) protocol_type: String,
    pub(crate) protocols: Vec<(String, Vec<u8>)>,
}

/// Each member's own protocol names, borrowed from `members` — the scratch
/// storage a caller must keep alive alongside `members` itself for as long
/// as the [`Candidate`] slices built over it are in use.
///
/// ⚠️ **Its own step, not folded into building `Candidate`s directly.**
/// `Candidate::protocols` is a borrowed slice (`&'a [&'a str]`), so its own
/// backing storage must outlive the `elect` call it feeds — a `Vec<&str>`
/// built and returned from *this* function lives in the caller's own scope,
/// where `elect` is then called, rather than inside a deeper helper whose
/// locals do not live that long.
fn protocol_names(members: &[RoundMember]) -> Vec<Vec<&str>> {
    members
        .iter()
        .map(|m| m.protocols.iter().map(|(name, _)| name.as_str()).collect())
        .collect()
}

/// Builds [`Candidate`]s over `members`, using `names` (this group's own
/// [`protocol_names`] output) for each one's own protocol list.
fn as_candidates<'a>(members: &'a [RoundMember], names: &'a [Vec<&'a str>]) -> Vec<Candidate<'a>> {
    members
        .iter()
        .zip(names)
        .map(|(m, protocols)| Candidate {
            member_id: m.member_id.as_str(),
            protocols: protocols.as_slice(),
        })
        .collect()
}

/// What a closed round answers every one of its own members with.
#[derive(Debug, Clone)]
pub(crate) struct RoundClose {
    pub(crate) generation: GenerationId,
    pub(crate) leader: String,
    pub(crate) protocol_type: String,
    pub(crate) protocol_name: String,
    pub(crate) members: Vec<RoundMember>,
}

/// A group's own currently-collecting round.
#[derive(Debug)]
struct OpenRound {
    members: Vec<RoundMember>,
    /// How many members close this round the moment they join, without
    /// waiting for `deadline` — `None` when nothing is known yet, which
    /// leaves `deadline` as the only way this round ever closes.
    expected: Option<usize>,
    deadline: Instant,
    notify: Arc<Notify>,
    outcome: Arc<OnceLock<Arc<RoundClose>>>,
}

/// One group's own join-round bookkeeping.
#[derive(Debug, Default)]
struct Entry {
    open: Option<OpenRound>,
    /// How many members closed this group's most recently *elected* round —
    /// the next round's own `expected`, until it closes and updates this.
    ///
    /// ⚠️ **`None` for a group that has never closed one — not `Some(1)`.**
    /// A group's true first-ever round has no prior membership to know a
    /// count from, so it waits out its own deadline like every other
    /// unresolved round rather than closing the instant its own opener
    /// joins; real Kafka's `group.initial.rebalance.delay.ms` names the
    /// same gap for the same reason (batching concurrently-starting
    /// consumers into one round), and this is that gap, stood in for by
    /// `rebalance_timeout_ms` itself rather than a second, not-yet-built
    /// configuration value.
    last_round_size: Option<usize>,
}

/// What [`GroupJoins::join`] hands back to a caller.
pub(crate) enum JoinOutcome {
    /// This join itself closed the round — every waiter, including the
    /// caller, has an answer already.
    Ready(Arc<RoundClose>),
    /// The round is still collecting; wait on `notify` or `deadline`,
    /// whichever comes first, then read `outcome`.
    Pending {
        notify: Arc<Notify>,
        deadline: Instant,
        outcome: Arc<OnceLock<Arc<RoundClose>>>,
    },
    /// This member was never enrolled — either it shares no protocol with
    /// the round's own running candidate set (`INCONSISTENT_GROUP_PROTOCOL`,
    /// the caller's job to map), or the coordinator's own transition table
    /// refused the event outright (`Dead`, unreachable in this milestone,
    /// this module's own bug rather than a case a client can trigger).
    Refused,
}

/// Every group's own join-round state, live for as long as this node is.
#[derive(Debug, Default)]
pub(crate) struct GroupJoins {
    entries: Mutex<HashMap<GroupId, Entry>>,
}

impl GroupJoins {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<GroupId, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Admits `member` to `group`'s own currently-open round, opening one
    /// (via `coordinator`'s own `Join`/`MemberJoinedDuringSync` event, per
    /// its current state) if none is open or the last one already closed.
    ///
    /// ⚠️ **The lock's own scope is this one call, tight and explicit** —
    /// `metadata_log.rs`'s own `clippy::significant_drop_tightening`
    /// precedent: the actual logic (several early returns) lives in
    /// [`join_locked`], a plain function over `&mut HashMap` the lock type
    /// never appears in, so nothing here obscures how long the guard lives.
    pub(crate) fn join(
        &self,
        coordinator: &dyn GroupCoordinator,
        group: &GroupId,
        member: RoundMember,
        rebalance_timeout: Duration,
    ) -> JoinOutcome {
        let mut entries = self.lock();
        let outcome = join_locked(&mut entries, coordinator, group, member, rebalance_timeout);
        drop(entries);
        outcome
    }

    /// Closes `group`'s own currently-open round if it has not already
    /// closed — a waiter's own deadline firing, `fetch::park`'s own
    /// deadline-wins-the-race shape. Idempotent: a round already closed
    /// (by reaching `expected`, or by a different waiter's own deadline)
    /// is left exactly as it was.
    ///
    /// ⚠️ **`own_outcome` is how a stale waiter is told its own round is
    /// gone, not the group's current one.** `entry.open` is *replaced* the
    /// moment a new round opens (`open_round`), keyed only by `GroupId` —
    /// so a waiter whose own round already closed *without* waking it (a
    /// `Notify::notify_waiters` call reaches only tasks already registered
    /// as waiters; one that has not yet reached its own `.notified().await`
    /// at that instant is not among them) would otherwise, on its own
    /// stale deadline, close whatever round is open *now* — a different
    /// group generation than the one this call ever joined. Comparing
    /// `own_outcome` against `entry.open`'s own `outcome` by pointer is
    /// what tells "my round, not yet closed" apart from "a round, but not
    /// mine": only the first may be closed here.
    pub(crate) fn close_on_deadline(
        &self,
        coordinator: &dyn GroupCoordinator,
        group: &GroupId,
        own_outcome: &Arc<OnceLock<Arc<RoundClose>>>,
    ) {
        let mut entries = self.lock();
        close_on_deadline_locked(&mut entries, coordinator, group, own_outcome);
        drop(entries);
    }
}

/// Opens a fresh round on `entry`, firing `coordinator`'s own `Join`/
/// `MemberJoinedDuringSync` event per its current state — [`join_locked`]'s
/// own "no round open, or the last one already closed" branch, pulled out
/// purely to keep that function under the fifty-line limit.
fn open_round(
    entry: &mut Entry,
    coordinator: &dyn GroupCoordinator,
    group: &GroupId,
    rebalance_timeout: Duration,
) -> Result<(), ()> {
    let event = match coordinator.record(group).map(|r| r.state) {
        None | Some(GroupState::Empty | GroupState::Stable) => GroupEvent::Join,
        Some(GroupState::CompletingRebalance) => GroupEvent::MemberJoinedDuringSync,
        // `PreparingRebalance`: this bookkeeping's own `entry.open` said no
        // round was open, so the coordinator agreeing a round is already in
        // flight means the two have diverged — this module's own bug, not a
        // case a client can trigger. `Dead`: unreachable, module doc above.
        Some(GroupState::PreparingRebalance | GroupState::Dead) => return Err(()),
    };
    if coordinator.transition(group, event).is_err() {
        return Err(());
    }
    entry.open = Some(OpenRound {
        members: Vec::new(),
        expected: entry.last_round_size,
        deadline: Instant::now() + rebalance_timeout,
        notify: Arc::new(Notify::new()),
        outcome: Arc::new(OnceLock::new()),
    });
    Ok(())
}

fn join_locked(
    entries: &mut HashMap<GroupId, Entry>,
    coordinator: &dyn GroupCoordinator,
    group: &GroupId,
    member: RoundMember,
    rebalance_timeout: Duration,
) -> JoinOutcome {
    let entry = entries.entry(group.clone()).or_default();

    if entry
        .open
        .as_ref()
        .is_none_or(|r| r.outcome.get().is_some())
        && open_round(entry, coordinator, group, rebalance_timeout).is_err()
    {
        return JoinOutcome::Refused;
    }

    let Some(round) = entry.open.as_mut() else {
        return JoinOutcome::Refused; // unreachable: just ensured `Some` above.
    };

    let mut trial = round.members.clone();
    if let Some(existing) = trial.iter_mut().find(|m| m.member_id == member.member_id) {
        *existing = member;
    } else {
        trial.push(member);
    }
    let names = protocol_names(&trial);
    if oqueue_core::elect(&as_candidates(&trial, &names)).is_none() {
        return JoinOutcome::Refused;
    }
    round.members = trial;

    let notify = Arc::clone(&round.notify);
    let deadline = round.deadline;
    let outcome = Arc::clone(&round.outcome);

    if round
        .expected
        .is_some_and(|expected| round.members.len() >= expected)
    {
        let close = close_locked(coordinator, group, round);
        entry.last_round_size = Some(close.members.len());
        let _ = outcome.set(Arc::clone(&close));
        notify.notify_waiters();
        return JoinOutcome::Ready(close);
    }

    JoinOutcome::Pending {
        notify,
        deadline,
        outcome,
    }
}

/// The logic [`GroupJoins::close_on_deadline`] runs under its own tight
/// lock scope — same split as [`join_locked`], same reason.
fn close_on_deadline_locked(
    entries: &mut HashMap<GroupId, Entry>,
    coordinator: &dyn GroupCoordinator,
    group: &GroupId,
    own_outcome: &Arc<OnceLock<Arc<RoundClose>>>,
) {
    let Some(entry) = entries.get_mut(group) else {
        return;
    };
    let Some(round) = entry.open.as_mut() else {
        return;
    };
    if !Arc::ptr_eq(&round.outcome, own_outcome) {
        return; // a round is open, but it is not the one this caller joined.
    }
    if round.outcome.get().is_some() {
        return; // already closed by another waiter.
    }
    let close = close_locked(coordinator, group, round);
    entry.last_round_size = Some(close.members.len());
    let _ = round.outcome.set(close);
    round.notify.notify_waiters();
}

/// Elects a leader/protocol over `round`'s own (guaranteed non-empty,
/// pairwise-compatible) members and closes it against `coordinator`.
///
/// # Panics
///
/// Never in practice: `round.members` is non-empty (a round only exists
/// once its first member has joined) and every member was admitted only
/// after `elect` confirmed it kept the round's own candidate set non-empty
/// — so `elect` here cannot return `None`. Falls back to the round's own
/// first member as a leaderless-protocol placeholder rather than panicking
/// if that invariant is ever violated, `security.md` rule 3's instinct.
fn close_locked(
    coordinator: &dyn GroupCoordinator,
    group: &GroupId,
    round: &mut OpenRound,
) -> Arc<RoundClose> {
    let members = std::mem::take(&mut round.members);
    let names = protocol_names(&members);
    let elected = oqueue_core::elect(&as_candidates(&members, &names));
    let (leader, protocol_name) = elected.map_or_else(
        || (members.first().map_or("", |m| m.member_id.as_str()), ""),
        |e| (e.leader, e.protocol_name),
    );
    let protocol_type = members
        .iter()
        .find(|m| m.member_id == leader)
        .map_or_else(String::new, |m| m.protocol_type.clone());
    // `JoinBarrierComplete` bumps the generation (`M4.1`'s own
    // `PreparingRebalance -> CompletingRebalance` arm) — refused only if
    // this bookkeeping and the coordinator have diverged (module doc
    // above), in which case the generation this round answers with is
    // whatever the coordinator already holds rather than a fabricated one.
    let generation = coordinator
        .transition(group, GroupEvent::JoinBarrierComplete)
        .map_or_else(
            |_| {
                coordinator
                    .record(group)
                    .map_or(GenerationId::INITIAL, |r| r.generation)
            },
            |r| r.generation,
        );
    Arc::new(RoundClose {
        generation,
        leader: leader.to_owned(),
        protocol_type,
        protocol_name: protocol_name.to_owned(),
        members,
    })
}

#[cfg(test)]
mod tests;
