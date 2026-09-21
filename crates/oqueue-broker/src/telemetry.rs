//! Fixed-schema operational events (`M12.10`, ADR-0065).

#![allow(clippy::redundant_pub_crate)]

use oqueue_codec::apikey::ApiKey;
use oqueue_core::Principal;
use tracing::Level;

pub(crate) fn admin_decision(
    api_key: ApiKey,
    outcome: &'static str,
    correlation_id: i32,
    principal: Option<&Principal>,
) {
    let Some(operation) = operation_name(api_key) else {
        return;
    };
    admin_operation(operation, outcome, correlation_id, principal, "cluster");
}

/// Emits the stable admin-operation schema. Dynamic values are deliberately
/// limited to an authenticated principal name, a protocol correlation ID, and
/// a bounded scope label; credentials and error strings never enter the event.
pub(crate) fn admin_operation(
    operation: &'static str,
    outcome: &'static str,
    correlation_id: i32,
    principal: Option<&Principal>,
    scope: &'static str,
) {
    tracing::event!(
        target: "oqueue",
        Level::INFO,
        event = "admin_operation",
        operation,
        outcome,
        correlation_id,
        principal = principal.map_or("anonymous", Principal::as_str),
        scope,
    );
}

/// Emits the stable dependency-failure schema without recording the dependency
/// error's display text, which may contain backend or key-provider details.
pub(crate) fn dependency_failure(
    dependency: &'static str,
    operation: &'static str,
    correlation_id: i32,
    principal: Option<&Principal>,
    scope: &'static str,
) {
    tracing::event!(
        target: "oqueue",
        Level::WARN,
        event = "dependency_failure",
        dependency,
        operation,
        outcome = "failure",
        correlation_id,
        principal = principal.map_or("anonymous", Principal::as_str),
        scope,
    );
}

const fn operation_name(api_key: ApiKey) -> Option<&'static str> {
    match api_key {
        ApiKey::CreateTopics => Some("create_topics"),
        ApiKey::DeleteTopics => Some("delete_topics"),
        ApiKey::DescribeConfigs => Some("describe_configs"),
        ApiKey::AlterConfigs | ApiKey::IncrementalAlterConfigs => Some("alter_configs"),
        ApiKey::DescribeGroups => Some("describe_groups"),
        ApiKey::ListGroups => Some("list_groups"),
        ApiKey::DescribeClientQuotas | ApiKey::AlterClientQuotas => Some("alter_quotas"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::significant_drop_tightening)]

    use super::{admin_decision, admin_operation, dependency_failure, operation_name};
    use oqueue_codec::apikey::ApiKey;
    use oqueue_core::Principal;
    use std::sync::{Arc, Mutex};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::{
        layer::{Context, Layer},
        prelude::*,
    };

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<Vec<&'static str>>>>);

    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = Names(Vec::new());
            event.record(&mut visitor);
            self.0.lock().expect("capture lock").push(visitor.0);
        }
    }

    struct Names(Vec<&'static str>);

    impl tracing::field::Visit for Names {
        fn record_debug(&mut self, field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {
            self.0.push(field.name());
        }
        fn record_i64(&mut self, field: &tracing::field::Field, _value: i64) {
            self.0.push(field.name());
        }
        fn record_str(&mut self, field: &tracing::field::Field, _value: &str) {
            self.0.push(field.name());
        }
    }

    #[test]
    fn events_have_fixed_safe_field_sets() {
        let fields = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Capture(Arc::clone(&fields)));
        let principal = Principal::new("alice").expect("valid principal");
        tracing::subscriber::with_default(subscriber, || {
            admin_operation("create_topics", "completed", 7, Some(&principal), "cluster");
            admin_decision(ApiKey::CreateTopics, "completed", 7, Some(&principal));
            dependency_failure("object_store", "put", 7, None, "produce");
        });
        let fields = fields.lock().expect("capture lock");
        assert_eq!(fields.len(), 3);
        assert_eq!(
            fields[0],
            vec![
                "event",
                "operation",
                "outcome",
                "correlation_id",
                "principal",
                "scope"
            ]
        );
        assert_eq!(
            fields[2],
            vec![
                "event",
                "dependency",
                "operation",
                "outcome",
                "correlation_id",
                "principal",
                "scope"
            ]
        );
    }

    #[test]
    fn every_admin_api_has_a_stable_operation_name() {
        let expected = [
            (ApiKey::CreateTopics, "create_topics"),
            (ApiKey::DeleteTopics, "delete_topics"),
            (ApiKey::DescribeConfigs, "describe_configs"),
            (ApiKey::AlterConfigs, "alter_configs"),
            (ApiKey::IncrementalAlterConfigs, "alter_configs"),
            (ApiKey::DescribeGroups, "describe_groups"),
            (ApiKey::ListGroups, "list_groups"),
            (ApiKey::DescribeClientQuotas, "alter_quotas"),
            (ApiKey::AlterClientQuotas, "alter_quotas"),
        ];
        for (api_key, name) in expected {
            assert_eq!(operation_name(api_key), Some(name));
        }
        assert_eq!(operation_name(ApiKey::Metadata), None);
    }
}
