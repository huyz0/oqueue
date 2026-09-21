//! Principal-scoped ownership of consumer groups (`M12.2`).

use crate::{GroupId, Principal};
use std::collections::{HashMap, HashSet};

/// Principal → consumer-group ownership.
///
/// Group ownership is separate from topic visibility: seeing a topic never
/// grants the ability to join, inspect, or mutate an unrelated group. An
/// empty policy denies every group operation when authentication is enabled.
#[derive(Debug, Default, Clone)]
pub struct GroupGrants {
    by_principal: HashMap<Principal, HashSet<GroupId>>,
}

impl GroupGrants {
    /// Creates a policy with no group ownership grants.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Grants one principal access to one consumer group.
    pub fn grant(&mut self, principal: Principal, group: GroupId) {
        self.by_principal
            .entry(principal)
            .or_default()
            .insert(group);
    }

    /// Returns whether the principal owns the group.
    #[must_use]
    pub fn allows(&self, principal: &Principal, group: &GroupId) -> bool {
        self.by_principal
            .get(principal)
            .is_some_and(|groups| groups.contains(group))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::GroupGrants;
    use crate::{GroupId, Principal, TopicGrants, TopicId};

    fn principal(name: &str) -> Principal {
        Principal::new(name).expect("valid principal")
    }

    fn group(name: &str) -> GroupId {
        GroupId::new(name).expect("valid group")
    }

    #[test]
    fn empty_group_policy_denies_by_default() {
        assert!(!GroupGrants::new().allows(&principal("alice"), &group("orders")));
    }

    #[test]
    fn topic_visibility_does_not_grant_group_ownership() {
        let mut visibility = TopicGrants::new();
        visibility.grant(
            principal("alice"),
            TopicId::new("orders").expect("valid topic"),
        );

        assert!(visibility.can_see(&principal("alice"), &TopicId::new("orders").expect("valid")));
        assert!(!GroupGrants::new().allows(&principal("alice"), &group("orders")));
    }

    #[test]
    fn one_principal_cannot_use_another_principals_group() {
        let mut grants = GroupGrants::new();
        grants.grant(principal("alice"), group("orders"));

        assert!(grants.allows(&principal("alice"), &group("orders")));
        assert!(!grants.allows(&principal("bob"), &group("orders")));
        assert!(!grants.allows(&principal("alice"), &group("payments")));
    }
}
