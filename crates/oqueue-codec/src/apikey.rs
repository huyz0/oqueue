//! The API keys this broker serves, and the header versions each implies.
//!
//! ⚠️ **`ADR-0019` moved this off `kafka-protocol`.** The dependency's
//! `ApiKey` knows every key Kafka defines; ours knows only the ones this
//! broker serves, so a key we do not implement is refused at the type
//! boundary rather than routed to a handler that would reject it — a tighter
//! surface, and the whole point of owning the layer.
//!
//! The header-version rules are the protocol's, expressed once here:
//! a **request** header is flexible (v2) exactly when the message is flexible
//! at that version, else v1; a **response** header is flexible (v1) on the
//! same condition — *except* `ApiVersions`, whose response header is always
//! v0, because a client must parse it before it knows the negotiated version
//! (doc 02 §7.6). That single exception is the one hand-computed rule the
//! whole protocol has, and [`crate::versions`]'s differential test pins our
//! answer to the dependency's generated one at every advertised version.

use crate::versions::advertised_for;

/// A Kafka API key this broker serves. The discriminant is the wire key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i16)]
pub enum ApiKey {
    /// Produce (0).
    Produce = 0,
    /// Fetch (1).
    Fetch = 1,
    /// `ListOffsets` (2).
    ListOffsets = 2,
    /// Metadata (3).
    Metadata = 3,
    /// `OffsetCommit` (8).
    OffsetCommit = 8,
    /// `FindCoordinator` (10).
    FindCoordinator = 10,
    /// `JoinGroup` (11).
    JoinGroup = 11,
    /// `Heartbeat` (12).
    Heartbeat = 12,
    /// `LeaveGroup` (13).
    LeaveGroup = 13,
    /// `SyncGroup` (14).
    SyncGroup = 14,
    /// `SaslHandshake` (17).
    SaslHandshake = 17,
    /// `ApiVersions` (18).
    ApiVersions = 18,
    /// `InitProducerId` (22).
    InitProducerId = 22,
    /// `SaslAuthenticate` (36).
    SaslAuthenticate = 36,
}

impl ApiKey {
    /// The wire key.
    #[must_use]
    pub const fn as_i16(self) -> i16 {
        self as i16
    }

    /// The `ApiKey` for a wire key, or `None` for one this broker does not
    /// serve — the caller's cue to close (a key we do not implement has no
    /// response the client would parse).
    #[must_use]
    pub const fn from_i16(key: i16) -> Option<Self> {
        match key {
            0 => Some(Self::Produce),
            1 => Some(Self::Fetch),
            2 => Some(Self::ListOffsets),
            3 => Some(Self::Metadata),
            8 => Some(Self::OffsetCommit),
            10 => Some(Self::FindCoordinator),
            11 => Some(Self::JoinGroup),
            12 => Some(Self::Heartbeat),
            13 => Some(Self::LeaveGroup),
            14 => Some(Self::SyncGroup),
            17 => Some(Self::SaslHandshake),
            18 => Some(Self::ApiVersions),
            22 => Some(Self::InitProducerId),
            36 => Some(Self::SaslAuthenticate),
            _ => None,
        }
    }

    /// Is `version` flexible (compact encodings, tagged fields) for this API?
    /// The cutover is the advertised table's `flexible_from`.
    #[must_use]
    pub fn is_flexible(self, version: i16) -> bool {
        advertised_for(self)
            .and_then(|a| a.flexible_from)
            .is_some_and(|from| version >= from)
    }

    /// The request header version: v2 when flexible, else v1.
    ///
    /// ⚠️ v0 (a bare prelude, no client id) is a legacy shape no advertised
    /// API uses, so it is not produced here — every request this broker
    /// serves carries at least a client id.
    #[must_use]
    pub fn request_header_version(self, version: i16) -> i16 {
        i16::from(self.is_flexible(version)) + 1
    }

    /// The response header version: v0 for `ApiVersions` always (the special
    /// case), else v1 when flexible, else v0.
    #[must_use]
    pub fn response_header_version(self, version: i16) -> i16 {
        if matches!(self, Self::ApiVersions) {
            0
        } else {
            i16::from(self.is_flexible(version))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ApiKey;

    #[test]
    fn wire_keys_round_trip_and_reject_the_unserved() {
        for key in [
            ApiKey::Produce,
            ApiKey::Fetch,
            ApiKey::ListOffsets,
            ApiKey::Metadata,
            ApiKey::OffsetCommit,
            ApiKey::FindCoordinator,
            ApiKey::JoinGroup,
            ApiKey::Heartbeat,
            ApiKey::LeaveGroup,
            ApiKey::SyncGroup,
            ApiKey::SaslHandshake,
            ApiKey::ApiVersions,
            ApiKey::InitProducerId,
            ApiKey::SaslAuthenticate,
        ] {
            assert_eq!(ApiKey::from_i16(key.as_i16()), Some(key));
        }
        // Keys Kafka defines but this broker does not serve. ⚠️ `2` was here
        // until `M3.21`, which added `ListOffsets`, `22` was here until
        // `M11.4`, which added `InitProducerId`, `10` was here until
        // `M4.3`, which added `FindCoordinator`, `11` was here until
        // `M4.7`, which added `JoinGroup`, and `17`/`36` were here
        // until `M9.3`, which added `SaslHandshake`/`SaslAuthenticate`: a
        // key moving from this list to the one above is what serving a new
        // API looks like, and leaving it in both is how the round trip
        // above starts lying. `8` (`OffsetCommit`, `M4.12`) never needed
        // that move -- it was never in this array to begin with.
        for unserved in [19, 20, -1, 32512] {
            assert_eq!(ApiKey::from_i16(unserved), None);
        }
    }

    #[test]
    fn header_versions_follow_the_flexible_cutover() {
        // Produce is flexible from v9.
        assert_eq!(ApiKey::Produce.request_header_version(8), 1);
        assert_eq!(ApiKey::Produce.request_header_version(9), 2);
        assert_eq!(ApiKey::Produce.response_header_version(8), 0);
        assert_eq!(ApiKey::Produce.response_header_version(9), 1);
        // ApiVersions: request header goes flexible at v3, but the response
        // header is v0 forever -- the special case.
        assert_eq!(ApiKey::ApiVersions.request_header_version(3), 2);
        assert_eq!(ApiKey::ApiVersions.response_header_version(3), 0);
        assert_eq!(ApiKey::ApiVersions.response_header_version(0), 0);
        // SaslHandshake: never flexible, at any advertised version --
        // module doc's own "standing example", falling out of
        // `flexible_from: None` rather than a second special case.
        assert_eq!(ApiKey::SaslHandshake.request_header_version(0), 1);
        assert_eq!(ApiKey::SaslHandshake.request_header_version(1), 1);
        assert_eq!(ApiKey::SaslHandshake.response_header_version(0), 0);
        assert_eq!(ApiKey::SaslHandshake.response_header_version(1), 0);
    }
}
