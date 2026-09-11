//! Whether this node has finished replaying the durable state it needs
//! before answering a group-protocol request honestly — `M4.15a`, FR-21.
//!
//! ⚠️ **The real signal `crate::fencing::NodeReadiness::load_in_progress`
//! was built for.** `fencing.rs`'s own module doc named this exactly:
//! "real in shape, not yet in trigger... `M4.15`'s own row is what wires a
//! genuine signal to it." `fence`'s own logic already checks the flag and
//! answers `COORDINATOR_LOAD_IN_PROGRESS` — this type is the flag, not a
//! second implementation of the check.
//!
//! ⚠️ **One direction only, and one thing it covers.** A gate only ever
//! moves from "still loading" to "ready," never back — a `Cluster` that
//! successfully replayed once does not un-replay. And it covers exactly
//! what `M4.15a` made durable-and-replayed at the time it was built: the
//! offset log (`M4.14`). `M4.15b`'s own row is what extends what feeds
//! this same gate to group-state replay too; nothing about the gate itself
//! needs to change for that; only what marks it ready does.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the
// `pub` clippy's `redundant_pub_crate` asks for — `fencing.rs`'s own
// precedent for the same standoff.
#![allow(clippy::redundant_pub_crate)]

use std::sync::atomic::{AtomicBool, Ordering};

/// A one-way "has this node's own replay finished" flag.
#[derive(Debug)]
pub(crate) struct ReplayGate {
    ready: AtomicBool,
}

impl ReplayGate {
    /// Not yet ready. Every `Cluster` starts here — its own background
    /// replay task has not had a chance to run at all yet, `tokio::spawn`'s
    /// own documented guarantee that a spawned task never runs inline with
    /// the task that spawned it.
    pub(crate) const fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
        }
    }

    /// Whether a request arriving right now should be refused
    /// `COORDINATOR_LOAD_IN_PROGRESS` rather than answered.
    pub(crate) fn still_loading(&self) -> bool {
        !self.ready.load(Ordering::Acquire)
    }

    /// Marks replay complete — `Release`, paired with [`Self::still_loading`]'s
    /// own `Acquire`, so a caller that observes `still_loading() == false`
    /// is guaranteed to also observe every write the replay itself made
    /// (`CommittedOffsets`'s own map) before this call, not just this flag.
    ///
    /// ⚠️ Idempotent by construction (a second call stores the same `true`
    /// again) — there is deliberately no `un_ready`, so a caller cannot
    /// accidentally reopen a window a later bug introduces.
    pub(crate) fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }
}

impl Default for ReplayGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ReplayGate;

    #[test]
    fn starts_loading_and_flips_once_marked_ready() {
        let gate = ReplayGate::new();
        assert!(gate.still_loading(), "a fresh gate has not replayed yet");

        gate.mark_ready();
        assert!(
            !gate.still_loading(),
            "still_loading must flip false once mark_ready is called"
        );

        // Idempotent: calling it again does not somehow reopen the window.
        gate.mark_ready();
        assert!(!gate.still_loading());
    }
}
