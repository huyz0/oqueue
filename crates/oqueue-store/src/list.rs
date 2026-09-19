//! `ObjectStore::list` for every `object_store`-backed backend (`M7.3a`).
//!
//! ⚠️ **Through `PaginatedListStore`, not `object_store::ObjectStore::list`.**
//! The latter treats its prefix as a path *segment* (`foo/bar` does not match
//! `foo/barbaz`), which is not the seam's plain string-prefix contract, and
//! answers a `Stream` this crate would need another dependency to poll. The
//! paginated API passes the prefix through verbatim, pushes `after` down as S3's
//! `start-after` and GCS's XML-API `start-after`, and bounds each page with `max_keys` —
//! one request per page, which is the cost the seam's caller is paying for.
//!
//! Shared by `s3.rs` and `gcs.rs` for the same reason `classify.rs` is: nothing
//! here is backend-specific.

// See `classify.rs`: the qualifier states the intended scope.
#![allow(clippy::redundant_pub_crate)]

use object_store::list::{PaginatedListOptions, PaginatedListStore};
use oqueue_core::{Error, ObjectKey, Result};

/// Lists up to `limit` keys under `prefix`, strictly after `after`, in byte
/// order.
///
/// ⚠️ **Filtered and sorted again here, defensively.** Both backends already
/// answer in lexical order from a start-after, but `object_store` documents the
/// order as unguaranteed; re-checking the prefix, the bound and the order costs
/// nothing a network round trip does not dwarf, and makes the contract this
/// crate's own rather than an SDK's.
///
/// # Errors
///
/// [`Error::Transient`] if the backend's own retries were exhausted,
/// [`Error::Permanent`] for anything else, including a listed name that is not
/// a valid [`ObjectKey`].
pub(crate) async fn list_keys<S: PaginatedListStore + ?Sized>(
    inner: &S,
    prefix: &str,
    after: Option<&ObjectKey>,
    limit: usize,
) -> Result<Vec<ObjectKey>> {
    let mut keys = Vec::new();
    let mut page_token = None;
    while keys.len() < limit {
        let options = PaginatedListOptions {
            offset: after.map(|key| key.as_str().to_owned()),
            max_keys: Some(limit - keys.len()),
            page_token,
            ..PaginatedListOptions::default()
        };
        let page = inner
            .list_paginated((!prefix.is_empty()).then_some(prefix), options)
            .await
            .map_err(|source| classify_list(&source))?;
        for object in page.result.objects {
            let key = ObjectKey::new(object.location.as_ref()).map_err(|_| Error::Permanent)?;
            if admits(&key, prefix, after) {
                keys.push(key);
            }
        }
        page_token = page.page_token;
        if page_token.is_none() {
            break;
        }
    }
    keys.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
    keys.dedup();
    keys.truncate(limit);
    Ok(keys)
}

/// Whether `key` belongs in a listing of `prefix` after `after`.
fn admits(key: &ObjectKey, prefix: &str, after: Option<&ObjectKey>) -> bool {
    key.as_str().starts_with(prefix) && after.is_none_or(|after| key.as_str() > after.as_str())
}

/// Classifies a failed list request.
///
/// ⚠️ **Its own function, not `classify`**, because a listing names no key:
/// `classify`'s `ObjectNotFound` and `PreconditionFailed` arms have nothing to
/// carry, and neither shape is one a list can produce. What is left is the
/// same split `classify` makes — a spent retry budget is `Transient`, anything
/// else is `Permanent`.
const fn classify_list(err: &object_store::Error) -> Error {
    if matches!(err, object_store::Error::Generic { .. }) {
        Error::Transient
    } else {
        Error::Permanent
    }
}

#[cfg(test)]
mod tests {
    // Same justification as `tls.rs`'s: every `expect` below is on a value
    // this module just constructed from a literal it controls.
    #![allow(clippy::expect_used)]

    use super::{admits, classify_list, list_keys};
    use object_store::Error as ObjErr;
    use object_store::list::{PaginatedListOptions, PaginatedListResult, PaginatedListStore};
    use object_store::path::Path;
    use object_store::{ListResult, ObjectMeta};
    use oqueue_core::{Error, ObjectKey};
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    fn key(name: &str) -> ObjectKey {
        ObjectKey::new(name).expect("a non-empty key")
    }

    /// Drives a future that never pends: the store below does no I/O.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    /// A backend that pages *below* `max_keys`: at most two objects a page,
    /// with the last key as its continuation token — the shape a real S3
    /// answers when it truncates a page for its own reasons.
    ///
    /// ⚠️ **Written against `async_trait`'s expansion by hand**, because this
    /// crate does not depend on `async_trait` and a test is no reason to.
    #[derive(Debug)]
    struct TwoPerPage(Vec<&'static str>);

    // `chrono` is not a dependency of this crate, so its `DateTime` cannot be
    // named for `DateTime::default()`; nothing reads the timestamp.
    #[allow(clippy::default_trait_access)]
    impl PaginatedListStore for TwoPerPage {
        fn list_paginated<'a, 'b, 't>(
            &'a self,
            prefix: Option<&'b str>,
            opts: PaginatedListOptions,
        ) -> Pin<Box<dyn Future<Output = object_store::Result<PaginatedListResult>> + Send + 't>>
        where
            'a: 't,
            'b: 't,
            Self: 't,
        {
            Box::pin(async move {
                let start = opts.page_token.or(opts.offset);
                let cap = opts.max_keys.unwrap_or(usize::MAX).min(2);
                let mut matching: Vec<&str> = self
                    .0
                    .iter()
                    .copied()
                    .filter(|k| k.starts_with(prefix.unwrap_or("")))
                    .filter(|k| start.as_deref().is_none_or(|s| *k > s))
                    .collect();
                matching.sort_unstable();
                let more = matching.len() > cap;
                matching.truncate(cap);
                let page_token = more.then(|| (*matching.last().expect("a page")).to_owned());
                let objects = matching
                    .into_iter()
                    .map(|k| ObjectMeta {
                        location: Path::from(k),
                        last_modified: Default::default(),
                        size: 0,
                        e_tag: None,
                        version: None,
                    })
                    .collect();
                Ok(PaginatedListResult {
                    result: ListResult {
                        common_prefixes: Vec::new(),
                        objects,
                        extensions: object_store::Extensions::default(),
                    },
                    page_token,
                })
            })
        }
    }

    fn five() -> TwoPerPage {
        TwoPerPage(vec!["t/5", "t/3", "u/1", "t/1", "t/4", "t/2"])
    }

    fn names(keys: &[ObjectKey]) -> Vec<&str> {
        keys.iter().map(ObjectKey::as_str).collect()
    }

    /// ⚠️ **Kills the `page_token` mutant**: without the token, the second
    /// request re-reads the first page, and the listing ends at two keys.
    #[test]
    fn a_backend_paging_below_the_limit_is_followed_to_the_end_once() {
        let listed = block_on(list_keys(&five(), "t/", None, 10)).expect("list succeeds");
        assert_eq!(names(&listed), ["t/1", "t/2", "t/3", "t/4", "t/5"]);
    }

    #[test]
    fn after_and_limit_hold_across_pages() {
        let after = key("t/1");
        let listed = block_on(list_keys(&five(), "t/", Some(&after), 3)).expect("list succeeds");
        assert_eq!(names(&listed), ["t/2", "t/3", "t/4"]);
        let listed = block_on(list_keys(&five(), "t/", None, 3)).expect("list succeeds");
        assert_eq!(names(&listed), ["t/1", "t/2", "t/3"]);
        assert!(
            block_on(list_keys(&five(), "t/", None, 0))
                .expect("list succeeds")
                .is_empty()
        );
    }

    #[test]
    fn a_key_outside_the_prefix_is_not_admitted() {
        assert!(admits(&key("t/a"), "t/", None));
        assert!(!admits(&key("u/a"), "t/", None));
    }

    #[test]
    fn after_is_strict() {
        let after = key("t/b");
        assert!(!admits(&key("t/b"), "t/", Some(&after)));
        assert!(!admits(&key("t/a"), "t/", Some(&after)));
        assert!(admits(&key("t/c"), "t/", Some(&after)));
    }

    #[test]
    fn a_spent_retry_budget_is_transient() {
        let raw = ObjErr::Generic {
            store: "S3",
            source: "blip".into(),
        };
        assert_eq!(classify_list(&raw), Error::Transient);
    }

    #[test]
    fn anything_else_is_permanent() {
        let raw = ObjErr::PermissionDenied {
            path: "t/".to_owned(),
            source: "denied".into(),
        };
        assert_eq!(classify_list(&raw), Error::Permanent);
    }
}
