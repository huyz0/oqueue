//! Fixed-schema operational events (`M12.10`, ADR-0065).

#![allow(clippy::redundant_pub_crate)]

use oqueue_codec::apikey::ApiKey;
use oqueue_core::Principal;
use tracing::{Level, Span};

pub(crate) fn operation_span(name: &'static str, scope: &'static str) -> Span {
    match name {
        "produce" => tracing::info_span!(
            target: "oqueue",
            "produce",
            operation = name,
            outcome = tracing::field::Empty,
            scope,
        ),
        "fetch" => tracing::info_span!(
            target: "oqueue",
            "fetch",
            operation = name,
            outcome = tracing::field::Empty,
            scope,
        ),
        _ => tracing::info_span!(
            target: "oqueue",
            "operation",
            operation = name,
            outcome = tracing::field::Empty,
            scope,
        ),
    }
}

pub(crate) fn dependency_span(
    dependency: &'static str,
    operation: &'static str,
    scope: &'static str,
) -> Span {
    tracing::info_span!(
        target: "oqueue",
        "dependency",
        dependency,
        operation,
        outcome = tracing::field::Empty,
        scope,
    )
}

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

    use super::{
        admin_decision, admin_operation, dependency_failure, dependency_span, operation_name,
        operation_span,
    };
    use oqueue_codec::apikey::ApiKey;
    use oqueue_core::Principal;
    use std::sync::{Arc, Mutex};
    use tracing::{Event, Id, Subscriber};
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

    type SpanRecord = (String, bool, Vec<&'static str>);

    #[derive(Clone)]
    struct Spans(Arc<Mutex<Vec<SpanRecord>>>);

    impl<S: Subscriber> Layer<S> for Spans {
        fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
            let parented = ctx.current_span().id().is_some_and(|current| current != id);
            self.0.lock().expect("span capture lock").push((
                attrs.metadata().name().to_owned(),
                parented,
                attrs
                    .metadata()
                    .fields()
                    .iter()
                    .map(|field| field.name())
                    .collect(),
            ));
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

    #[test]
    fn dependency_span_is_connected_and_has_only_safe_fields() {
        let spans = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Spans(Arc::clone(&spans)));
        tracing::subscriber::with_default(subscriber, || {
            let root = operation_span("produce", "produce");
            root.in_scope(|| {
                let child = dependency_span("object_store", "put", "produce");
                child.in_scope(|| {});
            });
            let fetch = operation_span("fetch", "fetch");
            fetch.in_scope(|| {});
        });
        let spans = spans.lock().expect("span capture lock");
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].0, "produce");
        assert!(!spans[0].1);
        assert_eq!(spans[1].0, "dependency");
        assert!(spans[1].1);
        assert_eq!(spans[0].2, vec!["operation", "outcome", "scope"]);
        assert_eq!(
            spans[1].2,
            vec!["dependency", "operation", "outcome", "scope"]
        );
        assert_eq!(spans[2].0, "fetch");
        assert!(!spans[2].1);
        assert_eq!(spans[2].2, vec!["operation", "outcome", "scope"]);
    }
}
