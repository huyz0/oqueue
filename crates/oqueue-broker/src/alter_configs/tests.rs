//! Focused `AlterConfigs` validation tests.

use super::replacement;
use oqueue_compact::DELETION_DELAY_MS;

#[test]
fn full_replacement_omission_resets_and_unknown_keys_fail() {
    assert_eq!(replacement(&[]), Ok(None));
    assert_eq!(
        replacement(&[oqueue_codec::alter_configs::AlterConfig {
            name: "retention.ms".to_owned(),
            value: Some((DELETION_DELAY_MS + 1).to_string()),
        }]),
        Ok(Some(DELETION_DELAY_MS + 1))
    );
    assert!(
        replacement(&[oqueue_codec::alter_configs::AlterConfig {
            name: "cleanup.policy".to_owned(),
            value: Some("compact".to_owned()),
        }])
        .is_err()
    );
}

#[test]
fn retention_shorter_than_the_gc_delay_is_rejected() {
    assert!(
        replacement(&[oqueue_codec::alter_configs::AlterConfig {
            name: "retention.ms".to_owned(),
            value: Some((DELETION_DELAY_MS - 1).to_string()),
        }])
        .is_err()
    );
}
