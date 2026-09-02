//! Per-principal in-flight-request bound (`security.md` rule 13, FR-45,
//! `M9.16`).

use crate::Principal;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// Bounds how many requests one principal may have in flight at once,
/// shared across every connection that principal opens.
///
/// ⚠️ **Per principal, not per connection.** A connection-scoped bound would
/// let one tenant multiply its share by opening more connections — exactly
/// the noisy-neighbor failure `security.md` rule 13 names. One
/// `PrincipalQuota` is built and its `Arc` cloned into every connection's
/// dispatcher sharing one broker — `Cluster`'s own identity-sharing, not
/// `PlainCredentials`'/`TopicGrants`' pattern of cloning the *data*, which
/// would give each connection its own independent counter and defeat the
/// bound entirely.
///
/// ⚠️ **Bounds concurrency, not a rate.** [`crate::RateGovernor`] is this
/// crate's sibling that shapes a *rate* of requests against an
/// object-storage backend's own ceiling; this type answers "how many
/// requests is this principal running right now" — rule 13's literal
/// wording ("in-flight requests"). A rate-based client quota is real,
/// useful work this type does not attempt, and v1 ships no wire-visible
/// quota-management API either (`DescribeClientQuotas`/`AlterClientQuotas`,
/// 48-49) — doc 02 §1.7 lists both as safely deferrable for an initial
/// broker; recorded as out of scope for `M9.16` rather than silently
/// absent.
#[derive(Debug)]
pub struct PrincipalQuota {
    max_in_flight: u32,
    in_flight: Mutex<HashMap<Principal, u32>>,
}

impl PrincipalQuota {
    /// A quota admitting at most `max_in_flight` concurrent requests per
    /// principal. `0` refuses every request outright — a real, if unusual,
    /// configuration, not a special case this constructor forbids.
    #[must_use]
    pub fn new(max_in_flight: u32) -> Self {
        Self {
            max_in_flight,
            in_flight: Mutex::default(),
        }
    }

    /// Attempts to admit one more in-flight request for `principal` against
    /// `quota`.
    ///
    /// `None` means `principal` is already at the bound; nothing was
    /// admitted and there is nothing to release. `Some` carries the one
    /// release — dropping it is the only way the count goes back down,
    /// `crate::Session`'s own precedent (via `oqueue-broker`) that a lock
    /// guards data, not control flow (`async-concurrency.md` rule 8),
    /// applied to a bound instead of an authenticated identity.
    ///
    /// A free function taking `quota` explicitly, rather than a method on
    /// `Arc<Self>`, so [`InFlight`]'s own `Arc` clone needs no receiver type
    /// beyond what stable Rust already gives plain `&self`/`&Arc<T>`.
    #[must_use]
    pub fn admit(quota: &Arc<Self>, principal: &Principal) -> Option<InFlight> {
        let mut held = quota.lock();
        let count = held.entry(principal.clone()).or_insert(0);
        if *count >= quota.max_in_flight {
            return None;
        }
        *count += 1;
        drop(held);
        Some(InFlight {
            quota: Arc::clone(quota),
            principal: principal.clone(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Principal, u32>> {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// One admitted request's hold on its principal's count — releases on drop,
/// success or panic alike, the connection task's own unwind-safety the same
/// as every other RAII guard in this workspace.
#[derive(Debug)]
pub struct InFlight {
    quota: Arc<PrincipalQuota>,
    principal: Principal,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut held = self.quota.lock();
        if let Some(count) = held.get_mut(&self.principal) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                held.remove(&self.principal);
            }
        }
    }
}

#[cfg(test)]
impl PrincipalQuota {
    /// How many principals this quota currently holds an entry for —
    /// test-only, since the whole point of removing a principal at zero
    /// (below) is that no production caller can observe it through
    /// `admit` alone: `or_insert(0)` behaves identically whether the entry
    /// already exists at zero or was never there.
    fn tracked_principals(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::PrincipalQuota;
    use crate::Principal;
    use std::sync::Arc;

    fn alice() -> Principal {
        Principal::new("alice").expect("valid")
    }

    fn bob() -> Principal {
        Principal::new("bob").expect("valid")
    }

    #[test]
    fn admits_up_to_the_bound() {
        let quota = Arc::new(PrincipalQuota::new(2));
        let a = alice();
        let first = PrincipalQuota::admit(&quota, &a);
        let second = PrincipalQuota::admit(&quota, &a);
        assert!(first.is_some());
        assert!(second.is_some());
    }

    #[test]
    fn refuses_the_request_over_the_bound() {
        let quota = Arc::new(PrincipalQuota::new(1));
        let a = alice();
        let _first = PrincipalQuota::admit(&quota, &a).expect("first is under the bound");
        assert!(
            PrincipalQuota::admit(&quota, &a).is_none(),
            "a second concurrent request must be refused at max_in_flight 1"
        );
    }

    #[test]
    fn releasing_frees_a_slot_for_the_same_principal() {
        let quota = Arc::new(PrincipalQuota::new(1));
        let a = alice();
        let first = PrincipalQuota::admit(&quota, &a).expect("first is under the bound");
        drop(first);
        assert!(
            PrincipalQuota::admit(&quota, &a).is_some(),
            "dropping the guard must release the slot it held"
        );
    }

    #[test]
    fn one_principal_at_its_bound_does_not_refuse_another() {
        let quota = Arc::new(PrincipalQuota::new(1));
        let a = alice();
        let b = bob();
        let _alice_in_flight = PrincipalQuota::admit(&quota, &a).expect("alice's first request");
        assert!(
            PrincipalQuota::admit(&quota, &b).is_some(),
            "bob's own count starts at zero, independent of alice's"
        );
    }

    #[test]
    fn releasing_the_last_slot_forgets_the_principal() {
        let quota = Arc::new(PrincipalQuota::new(1));
        let a = alice();
        let guard = PrincipalQuota::admit(&quota, &a).expect("admits under the bound");
        assert_eq!(quota.tracked_principals(), 1);
        drop(guard);
        assert_eq!(
            quota.tracked_principals(),
            0,
            "a principal with no in-flight requests left must not linger in the map"
        );
    }

    #[test]
    fn a_zero_bound_refuses_every_request() {
        let quota = Arc::new(PrincipalQuota::new(0));
        assert!(PrincipalQuota::admit(&quota, &alice()).is_none());
    }
}
