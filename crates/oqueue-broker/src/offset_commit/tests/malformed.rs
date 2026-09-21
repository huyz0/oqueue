use super::{handle, open, prelude};
use crate::connection::HandlerResponse;
use crate::testing::fixture;

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(
        &fixture.cluster,
        prelude(),
        &[0xFF; 3],
        &open(),
        &crate::authz::unconfigured_group_authz(),
    )
    .await;
    assert!(matches!(response, HandlerResponse::Close));
}
