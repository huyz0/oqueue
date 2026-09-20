//! Principal-scoped authority for administrative operations (`M12.1`).

use crate::Principal;
use std::collections::{HashMap, HashSet};

/// An administrative operation whose authority is distinct from topic
/// visibility. Seeing a topic never grants the ability to mutate or inspect
/// its configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdminOperation {
    /// Create a topic in the catalog.
    CreateTopics,
    /// Tombstone a topic in the catalog.
    DeleteTopics,
    /// Read topic configuration.
    DescribeConfigs,
    /// Replace or incrementally update topic configuration.
    AlterConfigs,
    /// Read consumer-group state.
    DescribeGroups,
    /// Enumerate consumer groups visible to the principal.
    ListGroups,
    /// Read or change the supported principal quota.
    AlterQuotas,
}

/// Principal → administrative-operation grants.
///
/// This is deliberately separate from [`crate::TopicGrants`]: a principal
/// may be able to read a topic without being allowed to create, delete, or
/// configure anything. An empty policy denies every operation.
#[derive(Debug, Default, Clone)]
pub struct AdminGrants {
    by_principal: HashMap<Principal, HashSet<AdminOperation>>,
}

impl AdminGrants {
    /// Creates a policy with no authority granted.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Grants one administrative operation to one principal.
    pub fn grant(&mut self, principal: Principal, operation: AdminOperation) {
        self.by_principal
            .entry(principal)
            .or_default()
            .insert(operation);
    }

    /// Returns whether the principal may perform the operation.
    #[must_use]
    pub fn allows(&self, principal: &Principal, operation: AdminOperation) -> bool {
        self.by_principal
            .get(principal)
            .is_some_and(|operations| operations.contains(&operation))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{AdminGrants, AdminOperation};
    use crate::{Principal, TopicGrants, TopicId};

    fn principal(name: &str) -> Principal {
        Principal::new(name).expect("valid principal")
    }

    #[test]
    fn empty_admin_policy_denies_by_default() {
        let policy = AdminGrants::new();
        assert!(!policy.allows(&principal("alice"), AdminOperation::CreateTopics));
    }

    #[test]
    fn topic_visibility_does_not_grant_admin_authority() {
        let mut visibility = TopicGrants::new();
        visibility.grant(
            principal("alice"),
            TopicId::new("orders").expect("valid topic"),
        );

        let admin = AdminGrants::new();
        assert!(visibility.can_see(&principal("alice"), &TopicId::new("orders").expect("valid")));
        assert!(!admin.allows(&principal("alice"), AdminOperation::DeleteTopics));
    }

    #[test]
    fn one_grant_does_not_authorize_other_operations() {
        let mut policy = AdminGrants::new();
        policy.grant(principal("alice"), AdminOperation::DescribeConfigs);

        assert!(policy.allows(&principal("alice"), AdminOperation::DescribeConfigs));
        assert!(!policy.allows(&principal("alice"), AdminOperation::AlterConfigs));
    }

    #[test]
    fn one_principals_grant_does_not_leak_into_another() {
        let mut policy = AdminGrants::new();
        policy.grant(principal("alice"), AdminOperation::CreateTopics);

        assert!(policy.allows(&principal("alice"), AdminOperation::CreateTopics));
        assert!(!policy.allows(&principal("bob"), AdminOperation::CreateTopics));
    }
}
