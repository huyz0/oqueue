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
/// The requested fetch offset is outside the partition's valid range (1).
pub const OFFSET_OUT_OF_RANGE: i16 = 1;
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
/// A write could not be made durable (19).
///
/// ⚠️ **Not a lie about replicas.** There are none here — durability is object
/// storage's, `ADR-0020` — but this is the code the protocol reserves for "the
/// write did not achieve its durability requirement", and every Kafka client
/// already retries it. A code meaning something else would be understood as
/// something else.
pub const NOT_ENOUGH_REPLICAS: i16 = 19;
/// This broker cannot serve the partition right now (5).
///
/// Here, a write that landed in object storage whose position could not be
/// journalled. It sends the client to `Metadata` and back, which is the right
/// response to a coordinator that is not answering.
pub const LEADER_NOT_AVAILABLE: i16 = 5;
/// The broker hit a condition it has no better code for (-1).
pub const UNKNOWN_SERVER_ERROR: i16 = -1;
/// The partition's offsets are not available for reading yet (78).
///
/// ⚠️ **The code a fetch answers when it cannot yet be as fresh as it has
/// promised.** Kafka defines it for a leader whose high watermark has not
/// caught up, which is the same shape: the partition exists and this broker
/// will serve it, just not at the freshness already committed to.
/// ⚠️ **Chosen over `LEADER_NOT_AVAILABLE` because of what clients do with
/// it**: the Java consumer's fetch error dispatch enumerates this code and
/// retries, and falls through to an `IllegalStateException` out of `poll()`
/// for codes it does not know — code 5 among them. A refusal a client turns
/// into a crash is worse than the wrong answer it was avoiding.
pub const OFFSET_NOT_AVAILABLE: i16 = 78;
/// The batch's compression codec is one this broker does not accept (76).
///
/// ⚠️ **Refused rather than trusted.** A compressed batch's records are behind
/// a codec nothing here implements, so the record count it declares cannot be
/// checked against the records it holds — and that count is what offsets are
/// allocated from. Decompression is `M8`'s, alongside the region header's
/// `alg` field (doc 10 #40).
pub const UNSUPPORTED_COMPRESSION_TYPE: i16 = 76;
/// The request is not one this broker can honour at all (42).
///
/// ⚠️ **`InitProducerId`'s transactional refusal, `M11.4`** — chosen over
/// `TRANSACTIONAL_ID_AUTHORIZATION_FAILED` (53), which the dependency
/// documents as non-retriable too but which claims an authorization system
/// this broker does not have; a client told "authorization failed" for a
/// feature that was never checked is the doc 13 §8 inversion by another
/// door. This code's own dependency description — "the message was sent to
/// an incompatible broker" — is exactly what a transactional producer
/// talking to a non-transactional one is, and both the Java and librdkafka
/// clients surface an unlisted `InitProducerId` code as a fatal error to
/// the application rather than retrying it.
pub const INVALID_REQUEST: i16 = 42;
/// A produced sequence number skips ahead of what this producer's line
/// expects next (45) — `M11.6`, `oqueue-coordinator`'s
/// `RejectReason::OutOfOrder` mapped to the wire.
pub const OUT_OF_ORDER_SEQUENCE_NUMBER: i16 = 45;
/// A produced sequence number repeats one already recorded, but the batch it
/// names is not the one that was recorded (46).
///
/// A stale retry, not the transparent-success replay case — `M11.6`,
/// `RejectReason::Duplicate` mapped to the wire.
pub const DUPLICATE_SEQUENCE_NUMBER: i16 = 46;
/// A producer's epoch is older than the one this broker has on record for
/// its id (47) — a zombie, not something a retry could mend. `M11.7`,
/// `RejectReason::StaleEpoch` mapped to the wire.
pub const INVALID_PRODUCER_EPOCH: i16 = 47;
/// The `SaslHandshake` mechanism the client asked for is not one this
/// broker enables (33) — `M9.3`; `ADR-0032`, only `PLAIN`.
pub const UNSUPPORTED_SASL_MECHANISM: i16 = 33;
/// A `SaslAuthenticate` exchange did not succeed (58) — `M9.3`'s own
/// pre-`M9.4` answer (no credential source configured yet) and `M9.4`'s
/// own answer to a real, wrong credential alike.
pub const SASL_AUTHENTICATION_FAILED: i16 = 58;
/// No protocol name is common to every member of a `JoinGroup` round (23).
///
/// `M4.6`'s own `elect` returning `None` mapped to the wire, `M4.7`'s own
/// job per `M4.6`'s backlog row: a member whose own advertised protocols
/// share nothing with the round's running intersection is refused this
/// code and never enrolled, rather than silently joining a group it could
/// never run an assignor with.
pub const INCONSISTENT_GROUP_PROTOCOL: i16 = 23;
/// An explicitly-named topic the requesting principal cannot `DESCRIBE` (29).
///
/// `M9.9`, one leg of `M9.1`'s verified finding against real Kafka source:
/// the *explicitly-named* case answers this rather than the silent omission
/// the *null-topic-array* case uses instead (`M9.10`).
pub const TOPIC_AUTHORIZATION_FAILED: i16 = 29;

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
        assert_eq!(
            super::OFFSET_OUT_OF_RANGE,
            ResponseError::OffsetOutOfRange.code()
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
            super::NOT_ENOUGH_REPLICAS,
            ResponseError::NotEnoughReplicas.code()
        );
        assert_eq!(
            super::LEADER_NOT_AVAILABLE,
            ResponseError::LeaderNotAvailable.code()
        );
        assert_eq!(
            super::UNKNOWN_SERVER_ERROR,
            ResponseError::UnknownServerError.code()
        );
        assert_eq!(
            super::UNSUPPORTED_COMPRESSION_TYPE,
            ResponseError::UnsupportedCompressionType.code()
        );
        assert_eq!(
            super::OFFSET_NOT_AVAILABLE,
            ResponseError::OffsetNotAvailable.code()
        );
        assert_eq!(
            super::UNKNOWN_TOPIC_ID,
            ResponseError::UnknownTopicId.code()
        );
        // NONE is the protocol's "no error" sentinel, which the dependency
        // represents as the absence of a ResponseError (code 0).
        assert_eq!(super::NONE, 0);
    }

    /// ⚠️ **Split out so the first differential test stays under fifty
    /// lines** (`code-structure.md`), not a claim these codes are checked
    /// differently — same oracle, same reasoning, added by `M11.4`/`M11.6`.
    #[test]
    fn the_idempotent_producer_codes_match_the_dependency() {
        assert_eq!(super::INVALID_REQUEST, ResponseError::InvalidRequest.code());
        assert_eq!(
            super::OUT_OF_ORDER_SEQUENCE_NUMBER,
            ResponseError::OutOfOrderSequenceNumber.code()
        );
        assert_eq!(
            super::DUPLICATE_SEQUENCE_NUMBER,
            ResponseError::DuplicateSequenceNumber.code()
        );
        assert_eq!(
            super::INVALID_PRODUCER_EPOCH,
            ResponseError::InvalidProducerEpoch.code()
        );
    }

    /// `M9.3`'s two SASL codes, same oracle.
    #[test]
    fn the_sasl_codes_match_the_dependency() {
        assert_eq!(
            super::UNSUPPORTED_SASL_MECHANISM,
            ResponseError::UnsupportedSaslMechanism.code()
        );
        assert_eq!(
            super::SASL_AUTHENTICATION_FAILED,
            ResponseError::SaslAuthenticationFailed.code()
        );
    }

    /// `M9.9`'s authorization code, same oracle.
    #[test]
    fn the_topic_authorization_code_matches_the_dependency() {
        assert_eq!(
            super::TOPIC_AUTHORIZATION_FAILED,
            ResponseError::TopicAuthorizationFailed.code()
        );
    }

    /// `M4.7`'s own `JoinGroup` code, same oracle.
    #[test]
    fn the_inconsistent_group_protocol_code_matches_the_dependency() {
        assert_eq!(
            super::INCONSISTENT_GROUP_PROTOCOL,
            ResponseError::InconsistentGroupProtocol.code()
        );
    }
}
