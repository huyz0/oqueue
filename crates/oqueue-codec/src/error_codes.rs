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
/// A record batch failed its CRC check or was otherwise unreadable (2).
pub const CORRUPT_MESSAGE: i16 = 2;
/// The topic or partition does not exist on this broker (3).
pub const UNKNOWN_TOPIC_OR_PARTITION: i16 = 3;
/// The `acks` value is not one the protocol defines (21).
pub const INVALID_REQUIRED_ACKS: i16 = 21;
/// The requested API version is not supported (35). The one code
/// `ApiVersions` answers with, in its v0-bodied fallback.
pub const UNSUPPORTED_VERSION: i16 = 35;
/// The message format (batch magic) is one this broker does not accept (43)
/// — v0/v1 batches, refused rather than converted (KIP-110).
pub const UNSUPPORTED_FOR_MESSAGE_FORMAT: i16 = 43;
/// A produced record is malformed — here, a records blob that is not
/// exactly one batch (87).
pub const INVALID_RECORD: i16 = 87;
/// The topic id does not exist on this broker (100).
pub const UNKNOWN_TOPIC_ID: i16 = 100;

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
        assert_eq!(
            super::UNKNOWN_TOPIC_OR_PARTITION,
            ResponseError::UnknownTopicOrPartition.code()
        );
        assert_eq!(super::CORRUPT_MESSAGE, ResponseError::CorruptMessage.code());
        assert_eq!(
            super::INVALID_REQUIRED_ACKS,
            ResponseError::InvalidRequiredAcks.code()
        );
        assert_eq!(
            super::UNSUPPORTED_FOR_MESSAGE_FORMAT,
            ResponseError::UnsupportedForMessageFormat.code()
        );
        assert_eq!(super::INVALID_RECORD, ResponseError::InvalidRecord.code());
        assert_eq!(
            super::UNKNOWN_TOPIC_ID,
            ResponseError::UnknownTopicId.code()
        );
        // NONE is the protocol's "no error" sentinel, which the dependency
        // represents as the absence of a ResponseError (code 0).
        assert_eq!(super::NONE, 0);
    }
}
