use super::Dispatcher;

pub(super) fn api_versions_response(
    prelude: oqueue_codec::frame::RequestPrelude,
    role: crate::health::NodeRole,
) -> Vec<u8> {
    let supported = oqueue_codec::versions::supports(
        oqueue_codec::apikey::ApiKey::ApiVersions,
        prelude.api_version,
    );
    // ⚠️ Unsupported versions fall back to v0, which every client can parse;
    // the error tells the client to retry at one this broker advertised.
    let (body_version, error_code) = if supported {
        (prelude.api_version, oqueue_codec::error_codes::NONE)
    } else {
        (0, oqueue_codec::error_codes::UNSUPPORTED_VERSION)
    };
    let mut out = Vec::new();
    if oqueue_codec::frame::encode_response_header(
        &mut out,
        oqueue_codec::apikey::ApiKey::ApiVersions,
        body_version,
        prelude.correlation_id,
    )
    .is_err()
    {
        out.clear();
    }
    let advertised: Vec<_> = oqueue_codec::versions::ADVERTISED
        .iter()
        .copied()
        .filter(|row| role.allows(row.api_key))
        .collect();
    oqueue_codec::apiversions::encode_response_for(&mut out, body_version, error_code, &advertised);
    out
}

impl Dispatcher {
    /// Selects the role whose protocol responsibilities this connection may reach.
    #[must_use]
    pub const fn with_role(mut self, role: crate::health::NodeRole) -> Self {
        self.role = role;
        self
    }
}
