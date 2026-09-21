use super::Dispatcher;
use crate::connection::HandlerResponse;
use oqueue_codec::frame::RequestPrelude;

impl Dispatcher {
    /// [`crate::authz::GroupAuthzContext`] for this connection, kept separate
    /// from topic visibility so one grant type cannot authorize the other.
    pub(super) fn group_authz_context<'a>(
        &'a self,
        principal: Option<&'a oqueue_core::Principal>,
    ) -> crate::authz::GroupAuthzContext<'a> {
        crate::authz::GroupAuthzContext {
            principal,
            credentials_configured: !self.credentials.is_empty(),
            group_grants: &self.group_grants,
        }
    }

    /// `FindCoordinator`'s own arm, pulled out of `dispatch`'s `match` for
    /// the same fifty-line-limit reason `metadata_handle` is.
    pub(super) fn find_coordinator_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::find_coordinator::handle(
            &self.cluster,
            prelude,
            body,
            &self.group_authz_context(principal.as_ref()),
        )
    }

    pub(super) async fn join_group_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::join_group::handle(
            &self.cluster,
            prelude,
            body,
            &self.group_authz_context(principal.as_ref()),
        )
        .await
    }

    pub(super) async fn sync_group_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::sync_group::handle(
            &self.cluster,
            prelude,
            body,
            &self.group_authz_context(principal.as_ref()),
        )
        .await
    }

    pub(super) async fn heartbeat_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::heartbeat::handle(
            &self.cluster,
            prelude,
            body,
            &self.group_authz_context(principal.as_ref()),
        )
        .await
    }

    pub(super) async fn leave_group_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::leave_group::handle(
            &self.cluster,
            prelude,
            body,
            &self.group_authz_context(principal.as_ref()),
        )
        .await
    }

    pub(super) fn describe_groups_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::group_admin::describe(
            &self.cluster,
            prelude,
            body,
            &self.group_authz_context(principal.as_ref()),
        )
    }

    pub(super) fn list_groups_handle(
        &self,
        prelude: RequestPrelude,
        body: &[u8],
    ) -> HandlerResponse {
        let principal = self.session.principal();
        crate::group_admin::list(
            &self.cluster,
            prelude,
            body,
            &self.group_authz_context(principal.as_ref()),
        )
    }
}
