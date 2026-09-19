//! `ObjectStore::list`'s conformance cases (`M7.3a`).
//!
//! ⚠️ **Every key lives under its own case's prefix**, and every assertion is
//! about that prefix alone: the suite runs twice against one `MinIO` bucket,
//! and the other cases leave objects behind, so a case that listed the whole
//! bucket would pass on an empty one and fail on the second run — the defect
//! `M1.21` found in a point-read case.

use super::{Case, Harness, block_on, c, key};
use oqueue_core::ObjectKey;

/// The listing cases, registered beside `cases()`'s own — split so that
/// function stays inside `too_many_lines`. None needs a capability: every
/// backend the suite runs lists.
pub(super) fn cases() -> Vec<Case> {
    vec![
        c(
            "list_answers_in_byte_order",
            |_| true,
            list_answers_in_byte_order,
        ),
        c(
            "list_pages_strictly_after",
            |_| true,
            list_pages_strictly_after,
        ),
        c(
            "list_isolates_by_string_prefix",
            |_| true,
            list_isolates_by_string_prefix,
        ),
        c(
            "list_of_an_unknown_prefix_is_empty",
            |_| true,
            list_of_an_unknown_prefix_is_empty,
        ),
        c("list_respects_limit", |_| true, list_respects_limit),
    ]
}

fn put_all(harness: &Harness, names: &[&str]) {
    for name in names {
        block_on(harness.store().put(&key(name), vec![1], None)).expect("put succeeds");
    }
}

fn list(harness: &Harness, prefix: &str, after: Option<&ObjectKey>, limit: usize) -> Vec<String> {
    block_on(harness.lister().list(prefix, after, limit))
        .expect("list succeeds")
        .iter()
        .map(|k| k.as_str().to_owned())
        .collect()
}

/// Byte order, not insertion order and not case-folded: `B` (0x42) sorts
/// before `a` (0x61), and `a` before `a0`.
pub(super) fn list_answers_in_byte_order(harness: &Harness) {
    put_all(
        harness,
        &["cl/order/b", "cl/order/a0", "cl/order/B", "cl/order/a"],
    );
    assert_eq!(
        list(harness, "cl/order/", None, 100),
        ["cl/order/B", "cl/order/a", "cl/order/a0", "cl/order/b"]
    );
}

/// Paging by the last key returned visits every key once, in order, and a
/// deleted key is not among them.
pub(super) fn list_pages_strictly_after(harness: &Harness) {
    put_all(
        harness,
        &[
            "cl/page/1",
            "cl/page/2",
            "cl/page/3",
            "cl/page/4",
            "cl/page/5",
            "cl/page/gone",
        ],
    );
    block_on(harness.store().delete(&[key("cl/page/gone")])).expect("delete succeeds");
    let mut seen = Vec::new();
    let mut after: Option<ObjectKey> = None;
    loop {
        let page = list(harness, "cl/page/", after.as_ref(), 2);
        assert!(page.len() <= 2, "a page is at most `limit`: {page:?}");
        let Some(last) = page.last() else { break };
        after = Some(key(last));
        seen.extend(page);
    }
    assert_eq!(
        seen,
        [
            "cl/page/1",
            "cl/page/2",
            "cl/page/3",
            "cl/page/4",
            "cl/page/5"
        ]
    );
    // `after` need not exist: a key between two stored ones starts the page
    // at the next stored key.
    assert_eq!(
        list(harness, "cl/page/", Some(&key("cl/page/2a")), 10),
        ["cl/page/3", "cl/page/4", "cl/page/5"]
    );
}

/// A string prefix, not a path segment: `cl/iso/a` takes `cl/iso/a/x` and
/// `cl/iso/ab` and nothing else; a trailing `/` narrows it to the directory.
pub(super) fn list_isolates_by_string_prefix(harness: &Harness) {
    put_all(harness, &["cl/iso/a/x", "cl/iso/ab", "cl/iso/b", "cl/isoa"]);
    assert_eq!(
        list(harness, "cl/iso/a", None, 10),
        ["cl/iso/a/x", "cl/iso/ab"]
    );
    assert_eq!(list(harness, "cl/iso/a/", None, 10), ["cl/iso/a/x"]);
}

/// A prefix nothing was written under answers an empty page, not an error.
pub(super) fn list_of_an_unknown_prefix_is_empty(harness: &Harness) {
    assert!(list(harness, "cl/never-written/", None, 10).is_empty());
}

/// `limit` bounds the page from the front of the order, and 0 answers empty.
pub(super) fn list_respects_limit(harness: &Harness) {
    put_all(harness, &["cl/limit/a", "cl/limit/b", "cl/limit/c"]);
    assert!(list(harness, "cl/limit/", None, 0).is_empty());
    assert_eq!(list(harness, "cl/limit/", None, 1), ["cl/limit/a"]);
    assert_eq!(
        list(harness, "cl/limit/", None, 2),
        ["cl/limit/a", "cl/limit/b"]
    );
}
