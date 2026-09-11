//! The one seam every `M4.7`-`M4.10` handler's own fencing decision routes
//! through — `M4.11`, FR-20, `error-handling.md` rule 3.
//!
//! ⚠️ **`M9.7`'s "one seam every caller passes through" precedent, applied
//! to fencing instead of authorization.** `crate::authz::topic_authorized`
//! is the shape this copies: a handler passes in what it knows, gets back a
//! decision, and never constructs one of the wire codes that decision can
//! produce by any other path — `check-fencing-seam.sh` is this module's own
//! `check-topic-list-scope.sh`.
//!
//! ⚠️ **Two of the five codes are unreachable in this milestone's own v1
//! architecture, on purpose, not by oversight.** `ADR-0033`: every group
//! resolves to this one node, unconditionally, so nothing in this
//! milestone's own scope can ever construct a [`FencingContext`] with
//! `is_coordinator: false` or `coordinator_available: false` — there is no
//! "wrong node" or "not ready" state a v1 deployment produces.
//! [`FencingContext::for_this_node`] bakes both to `true` for exactly that
//! reason: every real call site uses it, never the bare struct literal. The
//! bare struct literal still exists and [`fence`] still honours it, so a
//! later milestone that adds routing has the wire mapping already done
//! rather than inventing it from scratch — and so this module's own
//! table-driven test can prove each of the five codes for the input that
//! produces it, `join_group::round`'s own `GroupState::Dead` precedent for
//! testing an outcome nothing today reaches through a live handler.
//!
//! ⚠️ **`load_in_progress` is wired to a real signal — `M4.15a`.**
//! [`FencingContext::for_this_node`] now takes the caller's own
//! `still_loading` reading (`crate::cluster::Cluster::replay_in_progress`,
//! backed by `crate::replay_gate::ReplayGate`) instead of hardcoding
//! `false`. What that signal actually covers is narrower than the field's
//! own name might suggest: `M4.15a` gates on `M4.14`'s offset replay only,
//! since group *membership* state has no durable log of its own yet
//! (`M4.15b`, not yet built) — named here rather than left to look more
//! complete than it is.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the
// `pub` clippy's `redundant_pub_crate` asks for — `authz.rs`'s own
// precedent for the same standoff.
#![allow(clippy::redundant_pub_crate)]

use oqueue_codec::error_codes;
use oqueue_core::{GroupRecord, GroupState};

/// This node's own readiness to answer for a group at all — the three
/// flags [`FencingContext::for_this_node`] always sets to the same three
/// values in this milestone's own v1 architecture (module doc), grouped
/// into their own type so `FencingContext` does not carry a fourth,
/// unrelated bool (`member_tracked`) past `struct_excessive_bools`'s own
/// three-bool ceiling.
pub(crate) struct NodeReadiness {
    /// Whether this broker coordinates the named group at all —
    /// unconditionally `true` in this milestone (`ADR-0033`), module doc.
    pub(crate) is_coordinator: bool,
    /// Whether this broker, coordinating the group, is ready to answer for
    /// it — unconditionally `true` in this milestone, module doc.
    pub(crate) coordinator_available: bool,
    /// Whether this broker is still replaying durable state it needs
    /// before answering honestly — `crate::replay_gate::ReplayGate`'s own
    /// signal, threaded in by every real call site since `M4.15a`.
    pub(crate) load_in_progress: bool,
}

/// Every input a fencing decision needs, gathered so a handler passes one
/// value rather than five positional ones (`rust-style.md`'s argument-count
/// limit) — `crate::authz::AuthzContext`'s own precedent for the same
/// reason.
pub(crate) struct FencingContext<'a> {
    /// This node's own readiness to answer for the group at all.
    pub(crate) node: NodeReadiness,
    /// Whether the request's own claimed member id is one this broker's
    /// `heartbeat.rs`'s own tracking currently holds for this group.
    pub(crate) member_tracked: bool,
    /// The group's current coordinator record, if it has one.
    pub(crate) record: Option<&'a GroupRecord>,
    /// The request's own claimed generation, if the wire message carries
    /// one at all — `JoinGroupRequest`/`LeaveGroupRequest` do not, so a
    /// caller passes `None` rather than inventing a value to check.
    pub(crate) generation_id: Option<i32>,
    /// Which states the group may legitimately be in for this request to
    /// proceed, if the request has such a requirement at all — `None`
    /// skips the check entirely (`LeaveGroup`/`JoinGroup`'s own case: any
    /// live state may receive one).
    pub(crate) acceptable_states: Option<&'a [GroupState]>,
}

impl<'a> FencingContext<'a> {
    /// This node's own defaults (module doc): every real call site starts
    /// here rather than building a [`NodeReadiness`] at each one, and —
    /// more importantly — rather than being able to pass `false` for
    /// `is_coordinator`/`coordinator_available` by accident. Only
    /// `load_in_progress` varies by caller (`still_loading`); the other two
    /// stay `true`, `ADR-0033`'s "every group resolves to this node."
    pub(crate) const fn for_this_node(
        still_loading: bool,
        member_tracked: bool,
        record: Option<&'a GroupRecord>,
        generation_id: Option<i32>,
        acceptable_states: Option<&'a [GroupState]>,
    ) -> Self {
        Self {
            node: NodeReadiness {
                is_coordinator: true,
                coordinator_available: true,
                load_in_progress: still_loading,
            },
            member_tracked,
            record,
            generation_id,
            acceptable_states,
        }
    }
}

/// Why [`fence`] refused a request — one variant per wire code it may
/// produce, [`Refusal::error_code`] the only place any of them is ever
/// constructed (`check-fencing-seam.sh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    CoordinatorLoadInProgress,
    NotCoordinator,
    CoordinatorNotAvailable,
    UnknownMember,
    IllegalGeneration,
    RebalanceInProgress,
}

impl Refusal {
    /// The wire code this refusal answers with.
    pub(crate) const fn error_code(self) -> i16 {
        match self {
            Self::CoordinatorLoadInProgress => error_codes::COORDINATOR_LOAD_IN_PROGRESS,
            Self::NotCoordinator => error_codes::NOT_COORDINATOR,
            Self::CoordinatorNotAvailable => error_codes::COORDINATOR_NOT_AVAILABLE,
            Self::UnknownMember => error_codes::UNKNOWN_MEMBER_ID,
            Self::IllegalGeneration => error_codes::ILLEGAL_GENERATION,
            Self::RebalanceInProgress => error_codes::REBALANCE_IN_PROGRESS,
        }
    }
}

/// The one function every `M4.7`-`M4.10` handler's own fencing decision
/// routes through. `Ok(())` if `ctx` describes a request that may proceed;
/// `Err` names which of [`Refusal`]'s six outcomes refuses it.
///
/// Checked in this order — load/availability before anything
/// group-specific (a broker that cannot answer for a group at all has
/// nothing else worth checking), membership before generation, generation
/// before "rebalancing" (real Kafka's own broker-side precedence):
pub(crate) fn fence(ctx: &FencingContext<'_>) -> Result<(), Refusal> {
    if ctx.node.load_in_progress {
        return Err(Refusal::CoordinatorLoadInProgress);
    }
    if !ctx.node.is_coordinator {
        return Err(Refusal::NotCoordinator);
    }
    if !ctx.node.coordinator_available {
        return Err(Refusal::CoordinatorNotAvailable);
    }
    if !ctx.member_tracked {
        return Err(Refusal::UnknownMember);
    }
    // Defensive, not reachable through any real call site: `member_tracked`
    // is only ever `true` once `heartbeat.rs`'s own `register` has run,
    // which only ever follows a real coordinator record existing — the
    // same "refused rather than panicked on an internal invariant"
    // instinct `join_group::round`'s own `Dead` arm uses.
    let Some(record) = ctx.record else {
        return Err(Refusal::UnknownMember);
    };
    if let Some(generation_id) = ctx.generation_id
        && record.generation.get() != generation_id
    {
        return Err(Refusal::IllegalGeneration);
    }
    if let Some(states) = ctx.acceptable_states
        && !states.contains(&record.state)
    {
        return Err(Refusal::RebalanceInProgress);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
