use super::incremental_value;
use oqueue_codec::incremental_alter_configs::{
    ConfigOperation, IncrementalConfig, IncrementalConfigsResource,
};
use oqueue_compact::DELETION_DELAY_MS;

fn resource(
    name: &str,
    operation: ConfigOperation,
    value: Option<&str>,
) -> IncrementalConfigsResource {
    IncrementalConfigsResource {
        resource_type: 2,
        resource_name: "orders".to_owned(),
        configs: vec![IncrementalConfig {
            name: name.to_owned(),
            operation,
            value: value.map(str::to_owned),
        }],
    }
}

#[test]
fn scalar_operations_have_distinct_retention_semantics() {
    assert_eq!(
        incremental_value(&resource(
            "retention.ms",
            ConfigOperation::Set,
            Some(&(DELETION_DELAY_MS + 1).to_string()),
        )),
        Ok(Some(DELETION_DELAY_MS + 1))
    );
    assert_eq!(
        incremental_value(&resource("retention.ms", ConfigOperation::Delete, None)),
        Ok(None)
    );
    assert!(
        incremental_value(&resource(
            "retention.ms",
            ConfigOperation::Append,
            Some("1")
        ))
        .is_err()
    );
    assert!(
        incremental_value(&resource(
            "retention.ms",
            ConfigOperation::Subtract,
            Some("1")
        ))
        .is_err()
    );
}

#[test]
fn invalid_scalar_updates_are_rejected() {
    assert!(
        incremental_value(&resource(
            "cleanup.policy",
            ConfigOperation::Set,
            Some("compact")
        ))
        .is_err()
    );
    assert!(incremental_value(&resource("retention.ms", ConfigOperation::Set, None)).is_err());
    assert!(incremental_value(&resource("retention.ms", ConfigOperation::Set, Some("1"))).is_err());
}
