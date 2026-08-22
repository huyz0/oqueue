//! Kafka protocol error codes, as our own constants (`ADR-0019`).
//!
//! ⚠️ These are protocol constants — stable across every broker and client,
//! defined by Kafka's own error table — not anything a dependency owns. Each
//! is pinned to `kafka-protocol`'s `ResponseError` in the differential test
//! below, so a wrong number fails here rather than against a live client.
//! The set grows as each message codec (`M2.31`-`M2.34`) lands the errors it
//! answers with; a constant nothing uses yet is not added.

/// No error.
pub const NONE: i16 = 0;
/// The requested API version is not supported (35). The one code
/// `ApiVersions` answers with, in its v0-bodied fallback.
pub const UNSUPPORTED_VERSION: i16 = 35;

#[cfg(test)]
mod tests {
    use kafka_protocol::error::ResponseError;

    /// The `ADR-0019` oracle: our constants equal `kafka-protocol`'s.
    #[test]
    fn our_codes_match_the_dependency() {
        assert_eq!(
            super::UNSUPPORTED_VERSION,
            ResponseError::UnsupportedVersion.code()
        );
        // NONE is the protocol's "no error" sentinel, which the dependency
        // represents as the absence of a ResponseError (code 0).
        assert_eq!(super::NONE, 0);
    }
}
