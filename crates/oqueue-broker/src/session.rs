//! What one connection remembers between requests.
//!
//! ⚠️ **One thing, and it is hazard H2.** A producer that writes and then
//! immediately reads must see its own record. Doc 12 §4.6 calls that
//! read-your-writes and names it the second of the staleness hazards, because
//! the failure is silent: a successful poll returning no records, which every
//! client treats as "nothing was written".
//!
//! The mechanism is `ADR-0023`'s: a [`CommitAck`] carries a
//! [`SessionWatermark`] — a version **and the epoch it belongs to** — the
//! produce path stores it here, and the next fetch on this connection asks the
//! index to be at least that far before it answers.
//!
//! ⚠️ **A version alone would be the bug rather than the fix.** Two
//! incarnations of a coordinator count from their own beginnings, so a
//! watermark compared across a failover is comparing two unrelated counters —
//! and the half of that mistake that feels safe is the half that answers a
//! reader with data missing its own write. `CacheState::admits` refuses the
//! comparison outright when the epochs differ.
//!
//! [`CommitAck`]: oqueue_coordinator::CommitAck

use crate::cluster::Cluster;
use oqueue_coordinator::IndexWatch;
use oqueue_core::{Principal, ReadMode, RefreshReason, SessionWatermark};
use std::cmp::Ordering;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tokio::time::Instant;

/// One connection's memory.
///
/// ⚠️ A std `Mutex` (`async-concurrency.md` rules 6 and 8): the critical
/// section is a `Copy` read or write with no `.await` inside, and the lock
/// protects data rather than control flow. Requests on one connection are
/// pipelined and so genuinely concurrent, which is why there is a lock at all.
#[derive(Debug, Default)]
pub struct Session {
    watermark: Mutex<Option<SessionWatermark>>,
    /// The principal a successful `SaslAuthenticate` (`M9.4`) attached to
    /// this connection — `None` until then, `M9.7`'s own "the rule that
    /// every request carries one" made real. Once set it is not cleared or
    /// replaced: one connection authenticates at most once (`ADR-0032`
    /// carries no re-authentication story yet), and [`Session::authenticate`]
    /// below is what actually keeps that true — one lock acquisition
    /// covering both the check and the write.
    principal: Mutex<Option<Principal>>,
}

impl Session {
    /// Records `principal` as this connection's authenticated identity,
    /// unless one is already set. Returns whether `principal` is the one now
    /// held — `false` means an earlier exchange already won and `principal`
    /// was discarded.
    ///
    /// ⚠️ **One lock acquisition, not a read then a write.** `SaslAuthenticate`
    /// frames on one connection are genuinely concurrent — `connection.rs`
    /// dispatches every frame via its own `tokio::spawn`, and `bin/oqueue`'s
    /// shipped `max_in_flight` pipelines deeply by default — so a `principal()`
    /// read followed by a separate `authenticate` write would let two
    /// exchanges each observe "nothing set yet" before either writes,
    /// and the loser's write would still land. Checking and setting under the
    /// same held lock is what makes "at most once" true under that
    /// concurrency rather than only when nothing happens to pipeline.
    #[must_use]
    pub fn authenticate(&self, principal: Principal) -> bool {
        let mut held = self.lock_principal();
        if held.is_some() {
            return false;
        }
        *held = Some(principal);
        true
    }

    fn lock_principal(&self) -> std::sync::MutexGuard<'_, Option<Principal>> {
        self.principal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// This connection's authenticated identity, or `None` if
    /// `SaslAuthenticate` has not yet succeeded on it.
    #[must_use]
    pub fn principal(&self) -> Option<Principal> {
        self.lock_principal().clone()
    }

    /// Remembers `watermark` if it is newer than what this session holds.
    ///
    /// ⚠️ **Monotonic within an epoch, replaced across one.** Pipelined
    /// produces can ack out of order, so a bare store would let an older
    /// watermark overwrite a newer one and drop the freshness bar below a
    /// write this client has already been told about. A watermark from a
    /// *different* epoch is not older or newer — it is incomparable
    /// (`ADR-0023`) — and the newer incarnation is the one that matters, so it
    /// replaces.
    pub fn observe(&self, watermark: SessionWatermark) {
        let mut held = self.lock();
        // ⚠️ **Epoch first and by ordering, not by equality-then-compare.** A
        // version is only meaningful within its own incarnation (`ADR-0023`),
        // so the epochs decide first: an ack from an *older* one is stale
        // whatever its number, a *newer* one replaces whatever is held, and
        // only within one epoch does the version comparison mean anything.
        let keep = held.is_some_and(|current| match current.epoch().cmp(&watermark.epoch()) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => current.version() >= watermark.version(),
        });
        if !keep {
            *held = Some(watermark);
        }
    }

    /// The watermark this connection's next read must be at least as fresh as.
    #[must_use]
    pub fn watermark(&self) -> Option<SessionWatermark> {
        *self.lock()
    }

    /// Waits until the index holds whatever this connection has already been
    /// promised.
    ///
    /// ⚠️ **Bounded by the *staleness* budget, not by the client's
    /// `max_wait_ms`.** Read-your-writes is a correctness guarantee, and a
    /// non-blocking poll — `fetch.max.wait.ms = 0`, which every client sends
    /// routinely — would otherwise switch it off entirely: no wait, no
    /// re-check, and a partition answered `NONE` without the client's own
    /// write in it. That is hazard H2 delivered by the mechanism meant to
    /// prevent it. `ADR-0021`'s `MAX_METADATA_STALENESS_MS` is the system's
    /// own bound on how far behind a read may be, and it is the right one
    /// here because the question is how stale this broker may be, not how long
    /// this client is willing to block.
    ///
    /// ⚠️ **The decision is `CacheState::admits`, not a bare version compare**,
    /// and `ADR-0023` is why: a watermark carried across a failover belongs to
    /// another incarnation's counter, so it is not *behind* — it is
    /// incomparable, and comparing it would answer a reader with data missing
    /// its own write while looking satisfied.
    ///
    /// ⚠️ **`wait_for`'s answer is used**, which is the whole reason it returns
    /// a `bool`: "I waited" and "it arrived" are different, and for an
    /// `AtLeast(v)` read the difference is an answer versus hazard H2. Even a
    /// `true` is re-checked, because a version *number* from another epoch can
    /// be reached by this one without meaning anything.
    ///
    /// ⚠️ **In this broker the wait is always already over**, because the
    /// coordinator folds before it acks and the index here is that
    /// coordinator's. It becomes load-bearing when the reader is a different
    /// process (`M7`), and wiring it now is what makes that a configuration
    /// change rather than a correctness change.
    ///
    /// # Errors
    ///
    /// The [`RefreshReason`] that is still unmet when the budget runs out. A
    /// caller must **not** answer records then: a partition served short of a
    /// promise already made is the silent wrongness doc 12 §4.6 names.
    pub(crate) async fn catch_up(
        &self,
        cluster: &Cluster,
        watch: &mut IndexWatch,
    ) -> Result<(), RefreshReason> {
        let Some(want) = self.watermark() else {
            return Ok(());
        };
        let mode = ReadMode::AtLeast(want);
        let refusal = match cluster.cache_state().admits(mode, cluster.epoch()) {
            Ok(()) => return Ok(()),
            Err(reason) => reason,
        };
        let deadline =
            Instant::now() + Duration::from_millis(oqueue_core::MAX_METADATA_STALENESS_MS);
        // ⚠️ `select!` against the budget: a version this coordinator will
        // never reach — one from an incarnation that is gone — must cost a
        // bounded wait and a named failure, never a hang.
        let arrived = tokio::select! {
            () = tokio::time::sleep_until(deadline) => false,
            arrived = watch.wait_for(want.version()) => arrived,
        };
        if !arrived {
            return Err(refusal);
        }
        // ⚠️ **One wait, then one re-check, and no loop.** Reaching the version
        // and still being refused means the number arrived on a different line
        // (`ADR-0023`) or the stream has gone silent — and waiting longer
        // cannot mend either, so a loop here would be a way to spend the whole
        // budget discovering the same thing repeatedly.
        cluster.cache_state().admits(mode, cluster.epoch())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<SessionWatermark>> {
        self.watermark
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests;
