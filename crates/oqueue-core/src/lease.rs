//! Coordinator leadership, as a lease in object storage that its holder
//! polls (`M6.7`, `M6.md` tasks 11 and 13).
//!
//! ⚠️ **The holder fences itself on its own clock.** It records, locally, the
//! moment its lease stops being safe to act on — the start of its last
//! successful renewal plus [`LEASE_TTL_MS`] — and [`ObjectStoreLease::is_held`]
//! answers from that alone. A holder that was paused, partitioned or starved
//! past that moment finds itself fenced the instant it runs again, whether or
//! not any renewal failed loudly: the metastable case doc 13 §8 names, a node
//! that lost its lease and does not know.
//!
//! ⚠️ **And it polls, never waits to be told.** Every renewal first looks for
//! the next term; one that exists means a successor has taken over, and the
//! holder stops at once rather than at its deadline.
//!
//! ⚠️ **Terms are create-only objects**, `lease/<term>`, so two challengers
//! cannot both take one term. A challenger takes `term + 1` only once the
//! current term's recorded expiry plus [`LEASE_SKEW_MS`] has passed on its own
//! clock — by which time the holder's local deadline, never later than that
//! expiry on a clock at most the skew apart, has passed on its.
//!
//! ⚠️ **Neither the lease nor the log's create-only segments fences alone**
//! (`M6.17`, amending `ADR-0046` point 2). The segments refuse two writers
//! racing for one *live* key; pruning deletes keys, and a deleted key is
//! absent again, so a writer whose view predates a prune is not refused by
//! them. What closes that is the coordinator replaying from a freshly opened
//! view under each lease term before it writes — the lease decides who may
//! write, and the replay makes sure it writes where the log actually ends.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{
    ByteRange, Clock, Error, ObjectKey, ObjectStore, Precondition, PreconditionToken, Result,
};

/// How long a lease is valid after the renewal that granted it began.
///
/// ⚠️ **UNDERIVED**, and the floor of failover time: a successor waits it out
/// before it may take over from a holder that stopped renewing.
pub const LEASE_TTL_MS: i64 = 10_000;

/// How often a holder renews.
///
/// ⚠️ **A third of the TTL**, so two renewals can fail before the lease lapses.
pub const LEASE_RENEW_MS: i64 = 3_333;

/// How far two nodes' clocks may disagree, as the lease assumes.
///
/// ⚠️ **UNDERIVED**, and deliberately below `oqueue-compact`'s 10 s GC skew:
/// a lease that waited ten seconds more would double failover time to cover
/// a skew NTP-disciplined hosts do not have. A host outside it can overlap a
/// successor by the difference. ⚠️ **That overlap is unfenced once a prune
/// lands inside it**: the successor replays under its new term before
/// writing (`M6.17`), but the skewed old holder does not, and a key the
/// successor's checkpoint deleted is absent again for the old holder's
/// create-only write. The skew bound is an assumption this relies on.
pub const LEASE_SKEW_MS: i64 = 1_000;

const MAGIC: [u8; 4] = *b"OQLS";

/// The most probes one bisection may take before it is called a bug.
const SEARCH_PROBES: u32 = 128;

/// One node's handle on a shard's leadership lease.
pub struct ObjectStoreLease {
    store: Arc<dyn ObjectStore>,
    prefix: String,
    holder: String,
    clock: Arc<dyn Clock>,
    state: Mutex<Held>,
}

/// The term this node holds, if any, and until when it may act on it.
#[derive(Default)]
struct Held {
    term: Option<u64>,
    /// The handle the store returned for this node's last write of its term —
    /// an `ETag` or a generation, not a credential.
    write_handle: Option<PreconditionToken>,
    valid_until: i64,
}

/// ⚠️ Names the prefix and the term, never the holder's identity.
impl core::fmt::Debug for ObjectStoreLease {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let held = self.lock();
        f.debug_struct("ObjectStoreLease")
            .field("prefix", &self.prefix)
            .field("term", &held.term)
            .finish_non_exhaustive()
    }
}

impl ObjectStoreLease {
    /// A handle for `holder` on the lease under `prefix`; holds nothing yet.
    #[must_use]
    pub fn new(
        store: Arc<dyn ObjectStore>,
        prefix: impl Into<String>,
        holder: impl Into<String>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            prefix: prefix.into(),
            holder: holder.into(),
            clock,
            state: Mutex::new(Held::default()),
        }
    }

    /// Whether this node may act as leader *now*, on its own clock.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.clock.now().as_millis() < self.lock().valid_until
    }

    /// The term held, if any.
    #[must_use]
    pub fn term(&self) -> Option<u64> {
        self.lock().term
    }

    /// Takes the lease if it is free or has expired; `Ok(false)` if another
    /// node holds it.
    ///
    /// # Errors
    ///
    /// The store's error, or [`Error::MalformedMetadataSegment`] for a lease
    /// object that cannot be read.
    pub async fn acquire(&self) -> Result<bool> {
        let started = self.clock.now().as_millis();
        let next_term = match self.newest().await? {
            None => 0,
            Some((term, expires)) => {
                if started < expires.saturating_add(LEASE_SKEW_MS) {
                    return Ok(false);
                }
                term.checked_add(1).ok_or(Error::CommitVersionOverflow {
                    base: term,
                    delta: 1,
                })?
            }
        };
        let body = self.body(started.saturating_add(LEASE_TTL_MS));
        match self
            .store
            .put(&self.key(next_term)?, body, Some(Precondition::IfAbsent))
            .await
        {
            Ok(meta) => {
                let mut held = self.lock();
                held.term = Some(next_term);
                held.write_handle = Some(meta.precondition_token);
                held.valid_until = started.saturating_add(LEASE_TTL_MS);
                drop(held);
                Ok(true)
            }
            Err(Error::PreconditionFailed { .. }) => Ok(false),
            Err(other) => Err(other),
        }
    }

    /// Extends the lease; `Ok(false)` when it has been lost to a successor.
    ///
    /// ⚠️ **A failed renewal does not fence by itself** — the local deadline
    /// does, when it passes. A renewal that finds a successor's term fences
    /// at once.
    ///
    /// # Errors
    ///
    /// The store's error; the deadline stands.
    pub async fn renew(&self) -> Result<bool> {
        let started = self.clock.now().as_millis();
        let (Some(term), Some(token), deadline) = ({
            let held = self.lock();
            (held.term, held.write_handle.clone(), held.valid_until)
        }) else {
            return Ok(false);
        };
        // ⚠️ **A lapsed lease is not renewed** (`M6.7`'s review): past its
        // deadline a successor may already have read the old expiry, and a
        // renewal now would extend a lease two nodes believe they hold.
        if started >= deadline {
            self.lose();
            return Ok(false);
        }
        let successor = term.checked_add(1).ok_or(Error::CommitVersionOverflow {
            base: term,
            delta: 1,
        })?;
        if self.read(successor).await?.is_some() {
            self.lose();
            return Ok(false);
        }
        let body = self.body(started.saturating_add(LEASE_TTL_MS));
        match self
            .store
            .put(&self.key(term)?, body, Some(Precondition::IfMatches(token)))
            .await
        {
            Ok(meta) => {
                // ⚠️ **The new deadline counts only if the write landed in
                // time**: a successor reads the old expiry no earlier than the
                // deadline plus the skew on its clock, so a write finished
                // before the deadline minus the skew on this one was seen by
                // any successor that looked. One that finished later may not
                // have been, and the old deadline stands.
                let finished = self.clock.now().as_millis();
                let mut held = self.lock();
                held.write_handle = Some(meta.precondition_token);
                if finished < deadline.saturating_sub(LEASE_SKEW_MS) {
                    held.valid_until = started.saturating_add(LEASE_TTL_MS);
                }
                drop(held);
                Ok(true)
            }
            Err(Error::PreconditionFailed { .. }) => {
                self.lose();
                Ok(false)
            }
            Err(other) => Err(other),
        }
    }

    fn lose(&self) {
        let mut held = self.lock();
        held.valid_until = i64::MIN;
        held.write_handle = None;
        held.term = None;
    }

    fn lock(&self) -> MutexGuard<'_, Held> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn key(&self, term: u64) -> Result<ObjectKey> {
        ObjectKey::new(format!("{}/lease/{term:020}", self.prefix))
    }

    fn body(&self, expires: i64) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&expires.to_be_bytes());
        out.extend_from_slice(self.holder.as_bytes());
        out
    }

    /// A term's recorded expiry, or `None` if the term does not exist.
    async fn read(&self, term: u64) -> Result<Option<i64>> {
        let bytes = match self.store.get(&self.key(term)?, ByteRange::Full).await {
            Ok(bytes) => bytes,
            Err(Error::ObjectNotFound { .. }) => return Ok(None),
            Err(other) => return Err(other),
        };
        let malformed = Error::MalformedMetadataSegment { at: 0 };
        if bytes.get(..4) != Some(&MAGIC[..]) {
            return Err(malformed);
        }
        let expires: [u8; 8] = bytes
            .get(4..12)
            .and_then(|raw| raw.try_into().ok())
            .ok_or(malformed)?;
        Ok(Some(i64::from_be_bytes(expires)))
    }

    /// The newest term and its expiry — doubling then bisecting, as the log's
    /// base generations are found, since terms too are written contiguously.
    async fn newest(&self) -> Result<Option<(u64, i64)>> {
        let Some(mut expires) = self.read(0).await? else {
            return Ok(None);
        };
        let (mut present, mut absent) = (0_u64, 1_u64);
        while let Some(found) = self.read(absent).await? {
            expires = found;
            present = absent;
            // ⚠️ `checked_mul`, so the walk ends: a store answering every term
            // present would otherwise double to `u64::MAX` and probe it forever.
            absent = absent.checked_mul(2).ok_or(Error::CommitVersionOverflow {
                base: absent,
                delta: absent,
            })?;
        }
        // ⚠️ **Bounded**: a 64-bit space bisects in 64 probes, so 128 is room to
        // spare, and a search that has not converged by then is a bug that
        // fails rather than spins.
        for _ in 0..SEARCH_PROBES {
            if absent - present <= 1 {
                return Ok(Some((present, expires)));
            }
            let middle = present + (absent - present) / 2;
            if let Some(found) = self.read(middle).await? {
                expires = found;
                present = middle;
            } else {
                absent = middle;
            }
        }
        Err(Error::MalformedMetadataSegment { at: 0 })
    }
}
