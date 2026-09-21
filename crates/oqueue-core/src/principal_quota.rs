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
/// useful work this type does not attempt. M12 adds a small wire-visible
/// administration surface for the per-principal override map.
#[derive(Debug)]
pub struct PrincipalQuota {
    max_in_flight: u32,
    state: Mutex<QuotaState>,
}

#[derive(Debug, Default)]
struct QuotaState {
    in_flight: HashMap<Principal, u32>,
    overrides: HashMap<Principal, u32>,
}

impl PrincipalQuota {
    /// A quota admitting at most `max_in_flight` concurrent requests per
    /// principal. `0` refuses every request outright — a real, if unusual,
    /// configuration, not a special case this constructor forbids.
    #[must_use]
    pub fn new(max_in_flight: u32) -> Self {
        Self {
            max_in_flight,
            state: Mutex::default(),
        }
    }

    /// Returns the live limit for one principal, including an administrative
    /// override when one exists.
    #[must_use]
    pub fn limit_for(&self, principal: &Principal) -> u32 {
        self.lock()
            .overrides
            .get(principal)
            .copied()
            .unwrap_or(self.max_in_flight)
    }

    /// Sets or removes one principal's live override. `None` restores the
    /// constructor's default. The change is visible to every dispatcher that
    /// shares this quota immediately.
    pub fn set_override(&self, principal: Principal, limit: Option<u32>) {
        let mut state = self.lock();
        match limit {
            Some(limit) => {
                state.overrides.insert(principal, limit);
            }
            None => {
                state.overrides.remove(&principal);
            }
        }
    }

    /// Returns explicit per-principal overrides in stable order for a bounded
    /// administrative response.
    #[must_use]
    pub fn overrides(&self) -> Vec<(Principal, u32)> {
        let mut overrides: Vec<_> = self
            .lock()
            .overrides
            .iter()
            .map(|(principal, limit)| (principal.clone(), *limit))
            .collect();
        overrides.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        overrides
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
        let mut state = quota.lock();
        let limit = state
            .overrides
            .get(principal)
            .copied()
            .unwrap_or(quota.max_in_flight);
        let count = state.in_flight.entry(principal.clone()).or_insert(0);
        if *count >= limit {
            return None;
        }
        *count += 1;
        drop(state);
        Some(InFlight {
            quota: Arc::clone(quota),
            principal: principal.clone(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QuotaState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
        if let Some(count) = held.in_flight.get_mut(&self.principal) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                held.in_flight.remove(&self.principal);
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
        self.lock().in_flight.len()
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

    #[test]
    fn an_override_is_live_and_remove_restores_the_default() {
        let quota = Arc::new(PrincipalQuota::new(2));
        let a = alice();
        quota.set_override(a.clone(), Some(1));
        let _held = PrincipalQuota::admit(&quota, &a).expect("override admits one");
        assert!(PrincipalQuota::admit(&quota, &a).is_none());
        quota.set_override(a.clone(), None);
        assert_eq!(quota.limit_for(&a), 2);
        assert!(PrincipalQuota::admit(&quota, &a).is_some());
    }

    #[test]
    fn overrides_are_sorted_and_principal_scoped() {
        let quota = Arc::new(PrincipalQuota::new(4));
        quota.set_override(bob(), Some(3));
        quota.set_override(alice(), Some(2));
        assert_eq!(
            quota
                .overrides()
                .into_iter()
                .map(|(principal, limit)| (principal.as_str().to_owned(), limit))
                .collect::<Vec<_>>(),
            vec![("alice".to_owned(), 2), ("bob".to_owned(), 3)]
        );
    }
}
