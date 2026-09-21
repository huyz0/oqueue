//! Per-API handler adapters for [`super::Dispatcher`].

use super::Dispatcher;
use crate::connection::HandlerResponse;
use oqueue_codec::frame::RequestPrelude;

impl Dispatcher {
    /// `Metadata`'s own arm, pulled out of `dispatch`'s `match` purely to
    /// keep that function under the fifty-line limit.
    pub(super) async fn metadata_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        let topic_grants = self.topic_grants_snapshot(principal.as_ref());
        crate::metadata::handle(
            &self.cluster,
            prelude,
            body,
            &self.authz_context(principal.as_ref(), &topic_grants),
        )
        .await
    }

    /// `ListOffsets`'s own arm — `M9.12`'s per-principal scoping, same
    /// pattern as `metadata_handle`.
    pub(super) async fn listoffsets_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        let topic_grants = self.topic_grants_snapshot(principal.as_ref());
        crate::listoffsets::handle(
            &self.cluster,
            prelude,
            body,
            &self.authz_context(principal.as_ref(), &topic_grants),
        )
        .await
    }

    /// `Produce`'s own arm — `M9.12`'s per-principal scoping.
    pub(super) async fn produce_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        let topic_grants = self.topic_grants_snapshot(principal.as_ref());
        crate::produce::handle(
            &self.cluster,
            &self.session,
            prelude,
            body,
            &self.authz_context(principal.as_ref(), &topic_grants),
        )
        .await
    }

    /// `Fetch`'s own arm — `M9.12`'s per-principal scoping.
    pub(super) async fn fetch_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        let topic_grants = self.topic_grants_snapshot(principal.as_ref());
        crate::fetch::handle(
            &self.cluster,
            &self.session,
            prelude,
            body,
            &self.authz_context(principal.as_ref(), &topic_grants),
        )
        .await
    }

    /// `OffsetCommit`'s own arm — `M4.12`'s per-principal scoping and
    /// `M9.12`'s shared topic context.
    pub(super) async fn offset_commit_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        let topic_grants = self.topic_grants_snapshot(principal.as_ref());
        crate::offset_commit::handle(
            &self.cluster,
            prelude,
            body,
            &self.authz_context(principal.as_ref(), &topic_grants),
            &self.group_authz_context(principal.as_ref()),
        )
        .await
    }

    /// `OffsetFetch`'s own arm — `M4.13`'s per-principal scoping.
    pub(super) fn offset_fetch_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        let topic_grants = self.topic_grants_snapshot(principal.as_ref());
        crate::offset_fetch::handle(
            &self.cluster,
            prelude,
            body,
            &self.authz_context(principal.as_ref(), &topic_grants),
            &self.group_authz_context(principal.as_ref()),
        )
    }

    /// The `CreateTopics` admin arm shares the live topic-grant policy with
    /// every dispatcher created by the composition root.
    pub(super) async fn create_topics_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::create_topics::handle(
            &self.cluster,
            prelude,
            body,
            &self.admin_authz_context(principal.as_ref()),
            &self.topic_grants,
        )
        .await
    }

    /// The `DeleteTopics` admin arm shares the durable catalog and live
    /// visibility policy with every dispatcher.
    pub(super) async fn delete_topics_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::delete_topics::handle(
            &self.cluster,
            prelude,
            body,
            &self.admin_authz_context(principal.as_ref()),
            &self.topic_grants,
        )
        .await
    }

    /// The `DescribeConfigs` admin arm reports only settings with a live
    /// authoritative owner and scopes topic resources through visibility.
    pub(super) async fn describe_configs_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        let topic_grants = self.topic_grants_snapshot(principal.as_ref());
        crate::describe_configs::handle(
            &self.cluster,
            prelude,
            body,
            &self.admin_authz_context(principal.as_ref()),
            &self.authz_context(principal.as_ref(), &topic_grants),
        )
        .await
    }
}
