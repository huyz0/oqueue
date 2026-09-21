//! The principal → topic-set forward index (`M9.8`, FR-4, NFR-12).

use crate::{Principal, TopicId};
use std::collections::{HashMap, HashSet};

/// Which topics each principal can see (`M9.8`, FR-4, NFR-12).
///
/// Doc 15 §4's "genuine departure from how Kafka is built internally": a
/// `Metadata` request answered through this index costs O(topics this
/// principal can see), never O(topics that exist). Kafka's own broker holds
/// one cache and filters it per request instead (`M9.1`'s verified finding
/// against real Kafka source); this index inverts that, keyed by principal
/// from the start, so there is nothing to filter.
///
/// ⚠️ **This is the index, not its maintenance schedule.** `grant`/`revoke`
/// are the seam "topic create," "topic delete," and "ACL change" call
/// incrementally. `M12.3` now grants a successful creator visibility in the
/// shared live policy; deletion and explicit ACL APIs remain later work.
#[derive(Debug, Default, Clone)]
pub struct TopicGrants {
    by_principal: HashMap<Principal, HashSet<TopicId>>,
}

impl TopicGrants {
    /// An index with no grants at all.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Grants `principal` visibility into `topic`.
    ///
    /// ⚠️ **Touches only `principal`'s own entry.** No other principal's
    /// grants are read or written — the incremental maintenance doc 15 §4
    /// asks for, not a rebuild of the whole index on every change. Granting
    /// a topic `principal` can already see is a no-op (`HashSet::insert`'s
    /// own idempotence), not a duplicate.
    pub fn grant(&mut self, principal: Principal, topic: TopicId) {
        self.by_principal
            .entry(principal)
            .or_default()
            .insert(topic);
    }

    /// Revokes `principal`'s visibility into `topic` — the "topic delete"
    /// and "ACL change" triggers. A no-op, not an error, if `principal` could
    /// not see `topic` in the first place.
    pub fn revoke(&mut self, principal: &Principal, topic: &TopicId) {
        if let Some(topics) = self.by_principal.get_mut(principal) {
            topics.remove(topic);
        }
    }

    /// The topics `principal` can see, in no particular order — empty for a
    /// principal with no grants at all, the same as one with an empty entry:
    /// a caller cannot and need not distinguish "never granted anything" from
    /// "granted, then every grant revoked."
    pub fn topics_for(&self, principal: &Principal) -> impl Iterator<Item = &TopicId> {
        self.by_principal.get(principal).into_iter().flatten()
    }

    /// Whether `principal` can see `topic` specifically — `M9.9`'s
    /// explicitly-named-topic case (`TOPIC_AUTHORIZATION_FAILED`, not
    /// silence) needs a membership test, not just enumeration.
    #[must_use]
    pub fn can_see(&self, principal: &Principal, topic: &TopicId) -> bool {
        self.by_principal
            .get(principal)
            .is_some_and(|topics| topics.contains(topic))
    }

    /// Revokes a topic from every principal after a durable topic deletion.
    ///
    /// This is intentionally separate from [`Self::revoke`], whose principal
    /// argument is useful for an ACL edit but cannot express a resource
    /// deletion without first enumerating the whole policy.
    pub fn revoke_topic(&mut self, topic: &TopicId) {
        for topics in self.by_principal.values_mut() {
            topics.remove(topic);
        }
    }

    /// Copies only one principal's grants into a request-local policy.
    ///
    /// A dispatcher must not clone the global multi-tenant index for every
    /// request: NFR-12 charges the request for the topics this principal can
    /// see, not for every tenant's grants.
    #[must_use]
    pub fn for_principal(&self, principal: Option<&Principal>) -> Self {
        let mut scoped = Self::new();
        if let Some(principal) = principal {
            for topic in self.topics_for(principal) {
                scoped.grant(principal.clone(), topic.clone());
            }
        }
        scoped
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::TopicGrants;
    use crate::{Principal, TopicId};

    fn principal(name: &str) -> Principal {
        Principal::new(name).expect("valid")
    }

    fn topic(name: &str) -> TopicId {
        TopicId::new(name).expect("valid")
    }

    #[test]
    fn a_fresh_index_grants_nothing() {
        let index = TopicGrants::new();
        assert_eq!(index.topics_for(&principal("alice")).count(), 0);
        assert!(!index.can_see(&principal("alice"), &topic("t")));
    }

    #[test]
    fn a_granted_topic_is_visible() {
        let mut index = TopicGrants::new();
        index.grant(principal("alice"), topic("t"));
        assert!(index.can_see(&principal("alice"), &topic("t")));
        let seen: Vec<_> = index.topics_for(&principal("alice")).collect();
        assert_eq!(seen, vec![&topic("t")]);
    }

    #[test]
    fn revoking_a_topic_removes_it_from_every_principal() {
        let mut index = TopicGrants::new();
        let alice = principal("alice");
        let bob = principal("bob");
        let topic = topic("t");
        index.grant(alice.clone(), topic.clone());
        index.grant(bob.clone(), topic.clone());

        index.revoke_topic(&topic);

        assert!(!index.can_see(&alice, &topic));
        assert!(!index.can_see(&bob, &topic));
    }

    #[test]
    fn granting_the_same_topic_twice_does_not_duplicate_it() {
        let mut index = TopicGrants::new();
        index.grant(principal("alice"), topic("t"));
        index.grant(principal("alice"), topic("t"));
        assert_eq!(index.topics_for(&principal("alice")).count(), 1);
    }

    #[test]
    fn revoking_removes_visibility() {
        let mut index = TopicGrants::new();
        index.grant(principal("alice"), topic("t"));
        index.revoke(&principal("alice"), &topic("t"));
        assert!(!index.can_see(&principal("alice"), &topic("t")));
        assert_eq!(index.topics_for(&principal("alice")).count(), 0);
    }

    #[test]
    fn revoking_a_topic_never_granted_is_a_no_op() {
        let mut index = TopicGrants::new();
        index.revoke(&principal("alice"), &topic("t"));
        assert!(!index.can_see(&principal("alice"), &topic("t")));
    }

    #[test]
    fn revoking_one_principals_topic_never_granted_does_not_touch_another() {
        let mut index = TopicGrants::new();
        index.grant(principal("alice"), topic("t"));
        index.revoke(&principal("bob"), &topic("t"));
        assert!(
            index.can_see(&principal("alice"), &topic("t")),
            "unrelated principal's revoke leaves alice's grant alone"
        );
    }

    /// ⚠️ **The isolation property FR-40 exists for, at the index's own
    /// level** — `M9.13`'s wire-level cross-principal test is this same
    /// claim exercised through a real `Metadata` request; here it is
    /// exercised directly, cheaply, without a broker.
    #[test]
    fn two_principals_grants_do_not_leak_into_each_other() {
        let mut index = TopicGrants::new();
        index.grant(principal("alice"), topic("alice-only"));
        index.grant(principal("bob"), topic("bob-only"));

        assert!(index.can_see(&principal("alice"), &topic("alice-only")));
        assert!(!index.can_see(&principal("alice"), &topic("bob-only")));
        assert!(index.can_see(&principal("bob"), &topic("bob-only")));
        assert!(!index.can_see(&principal("bob"), &topic("alice-only")));
    }

    #[test]
    fn a_principal_may_see_more_than_one_topic() {
        let mut index = TopicGrants::new();
        index.grant(principal("alice"), topic("a"));
        index.grant(principal("alice"), topic("b"));
        let mut seen: Vec<_> = index
            .topics_for(&principal("alice"))
            .map(TopicId::as_str)
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, vec!["a", "b"]);
    }

    /// ⚠️ **Not a timing assertion** — NFR-12's actual cost test is `M9.17`'s.
    /// This is the correctness half only: a principal with few grants sees
    /// exactly its own, regardless of how many other principals or topics the
    /// index otherwise holds.
    #[test]
    fn a_principals_own_query_is_unaffected_by_how_many_other_grants_exist() {
        let mut index = TopicGrants::new();
        for n in 0..10_000 {
            index.grant(principal(&format!("tenant-{n}")), topic(&format!("t{n}")));
        }
        index.grant(principal("alice"), topic("alice-1"));
        index.grant(principal("alice"), topic("alice-2"));

        assert_eq!(index.topics_for(&principal("alice")).count(), 2);
    }

    #[test]
    fn a_scoped_copy_contains_only_the_requested_principal() {
        let mut index = TopicGrants::new();
        index.grant(principal("alice"), topic("alice-only"));
        index.grant(principal("bob"), topic("bob-only"));

        let alice = principal("alice");
        let scoped = index.for_principal(Some(&alice));
        assert!(scoped.can_see(&alice, &topic("alice-only")));
        assert!(!scoped.can_see(&alice, &topic("bob-only")));
        assert_eq!(scoped.topics_for(&principal("bob")).count(), 0);
    }
}
