//! Role-aware health and readiness derived from live broker signals.

use crate::cluster::Cluster;

/// The responsibility label carried by a health snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeRole {
    /// A node that currently owns both coordinator and data-plane duties.
    Combined,
    /// A node responsible for metadata coordination.
    Coordinator,
    /// A node responsible for serving data-plane requests.
    DataPlane,
}

/// The broad operator-facing state of a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    /// Required replay is complete and no dependency failure is observed.
    Ready,
    /// The node can still route requests, but an observed dependency is
    /// failing or a customer-key domain is unavailable.
    Degraded,
    /// The node cannot honestly serve its role yet or its coordinator task
    /// has stopped.
    NotReady,
}

/// A point-in-time health/readiness response with no secret or topic fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthSnapshot {
    /// The configured responsibility label.
    pub role: NodeRole,
    /// The classified operator-facing state.
    pub state: HealthState,
    /// Whether durable replay is still in progress.
    pub replay_in_progress: bool,
    /// Whether the coordinator/group-transition task is still alive.
    pub coordinator_task_alive: bool,
    /// Number of observed object-storage failures.
    pub storage_failures: u64,
    /// Number of observed coordinator replay failures.
    pub coordinator_failures: u64,
    /// Number of observed customer-key-domain failures.
    pub key_domain_failures: u64,
}

impl Cluster {
    /// Returns role-aware health from the replay task and bounded metrics.
    ///
    /// This is observational: a zero failure count means no failure has been
    /// observed, not that a new storage probe was issued. KMS-domain failures
    /// remain `Degraded`, so one affected key domain cannot mark unrelated
    /// data unavailable.
    #[must_use]
    pub fn health(&self) -> HealthSnapshot {
        let metrics = self.metrics().snapshot();
        let replay_in_progress = self.replay_in_progress();
        let coordinator_task_alive = self.group_transitions_task_alive();
        let state = classify(
            replay_in_progress,
            coordinator_task_alive,
            metrics.storage_failures,
            metrics.coordinator_failures,
            metrics.key_domain_failures,
        );
        HealthSnapshot {
            role: self.role(),
            state,
            replay_in_progress,
            coordinator_task_alive,
            storage_failures: metrics.storage_failures,
            coordinator_failures: metrics.coordinator_failures,
            key_domain_failures: metrics.key_domain_failures,
        }
    }
}

const fn classify(
    replay_in_progress: bool,
    coordinator_task_alive: bool,
    storage_failures: u64,
    coordinator_failures: u64,
    key_domain_failures: u64,
) -> HealthState {
    if replay_in_progress || !coordinator_task_alive {
        HealthState::NotReady
    } else if storage_failures != 0 || coordinator_failures != 0 || key_domain_failures != 0 {
        HealthState::Degraded
    } else {
        HealthState::Ready
    }
}

#[cfg(test)]
mod tests {
    use super::{HealthState, classify};

    #[test]
    fn replay_or_dead_coordinator_is_not_ready() {
        assert_eq!(classify(true, true, 0, 0, 0), HealthState::NotReady);
        assert_eq!(classify(false, false, 0, 0, 0), HealthState::NotReady);
    }

    #[test]
    fn observed_failures_degrade_without_taking_the_node_down() {
        assert_eq!(classify(false, true, 1, 0, 0), HealthState::Degraded);
        assert_eq!(classify(false, true, 0, 1, 0), HealthState::Degraded);
        assert_eq!(classify(false, true, 0, 0, 1), HealthState::Degraded);
    }

    #[test]
    fn no_failures_after_replay_is_ready() {
        assert_eq!(classify(false, true, 0, 0, 0), HealthState::Ready);
    }
}
