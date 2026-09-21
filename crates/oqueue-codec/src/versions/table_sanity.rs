use super::ADVERTISED;
use std::collections::HashSet;

#[test]
fn the_table_is_unique_and_sane() {
    let mut keys = HashSet::new();
    for row in ADVERTISED {
        assert!(keys.insert(row.api_key), "no duplicate API keys");
    }
    for row in ADVERTISED {
        assert!(
            row.min <= row.max,
            "{:?} has an inverted range",
            row.api_key
        );
        if let Some(from) = row.flexible_from {
            assert!(
                from >= row.min && from <= row.max + 1,
                "{:?}'s cutover must touch its advertised range",
                row.api_key
            );
        }
    }
}
