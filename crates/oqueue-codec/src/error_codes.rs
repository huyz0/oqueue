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
/// The request exceeded its client-supplied deadline (7).
pub const REQUEST_TIMED_OUT: i16 = 7;
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
/// The requested configuration is not supported by this broker (40).
pub const INVALID_CONFIG: i16 = 40;
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
/// The group is rebalancing; rejoin rather than treat this as accepted (27).
///
/// `M4.9`'s own answer to a `Heartbeat` whose `generation_id` does not
/// match the coordinator's current one — the group has moved on (a new
/// round opened, or this member was itself evicted for it), and the client
/// is told to `JoinGroup` again rather than being silently accepted into a
/// generation it is not part of. Finer fencing (`ILLEGAL_GENERATION` for a
/// member that never belonged at all) is `M4.11`'s own audited path, not
/// this task's to distinguish.
pub const REBALANCE_IN_PROGRESS: i16 = 27;
/// An explicitly-named topic the requesting principal cannot `DESCRIBE` (29).
///
/// `M9.9`, one leg of `M9.1`'s verified finding against real Kafka source:
/// the *explicitly-named* case answers this rather than the silent omission
/// the *null-topic-array* case uses instead (`M9.10`).
pub const TOPIC_AUTHORIZATION_FAILED: i16 = 29;
/// The topic already exists with the requested name (36).
pub const TOPIC_ALREADY_EXISTS: i16 = 36;
/// The requested partition count is invalid (37).
pub const INVALID_PARTITIONS: i16 = 37;
/// The requested replication factor is invalid (38).
pub const INVALID_REPLICATION_FACTOR: i16 = 38;
/// The topic name or topic request is invalid (17).
pub const INVALID_TOPIC_EXCEPTION: i16 = 17;
/// The requesting principal does not own the named consumer group (30).
pub const GROUP_AUTHORIZATION_FAILED: i16 = 30;
/// A group member id this broker's own tracking does not recognise (25).
///
/// `M4.11`'s own audited fencing path, `crate::fencing`'s `Refusal` — the
/// only place this constant is ever constructed. Distinguished from
/// [`ILLEGAL_GENERATION`] by what is stale: this member id was never (or
/// no longer is) tracked at all, rather than tracked but naming a round
/// that has since moved on.
pub const UNKNOWN_MEMBER_ID: i16 = 25;
/// A group member's own claimed generation does not match the coordinator's
/// current one (22).
///
/// `M4.11`'s own audited fencing path — a member this broker still tracks,
/// naming a generation that is not (or is no longer) the group's current
/// one.
pub const ILLEGAL_GENERATION: i16 = 22;
/// This broker does not coordinate the named group (16).
///
/// ⚠️ **Unreachable in this milestone's own v1 architecture** —
/// `ADR-0033`: every group resolves to this one node, unconditionally, so
/// no v1 deployment can ever be asked about a group it does not host. The
/// code exists, and `M4.11`'s own fencing seam accepts the input that
/// would produce it, so a later milestone that adds routing does not have
/// to invent the wire mapping from scratch — but nothing in this
/// milestone's own scope ever passes that input for real.
pub const NOT_COORDINATOR: i16 = 16;
/// This broker coordinates the named group but cannot answer for it right
/// now (15).
///
/// ⚠️ Unreachable for the identical reason [`NOT_COORDINATOR`] is —
/// `ADR-0033`'s single-coordinator v1 architecture has no "not ready yet"
/// state today.
pub const COORDINATOR_NOT_AVAILABLE: i16 = 15;
/// This broker is still replaying the named group's own state and cannot
/// answer for it yet (14).
///
/// ⚠️ Real in shape, not yet in trigger — `M4.15`'s own row names this
/// exact code as what it wires a genuine signal to (durable group-state
/// replay on coordinator takeover). Every caller in this milestone passes
/// `load_in_progress: false`.
pub const COORDINATOR_LOAD_IN_PROGRESS: i16 = 14;

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
        assert_eq!(super::INVALID_CONFIG, ResponseError::InvalidConfig.code());
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

    #[test]
    fn create_topic_codes_match_the_dependency() {
        assert_eq!(
            super::TOPIC_ALREADY_EXISTS,
            ResponseError::TopicAlreadyExists.code()
        );
        assert_eq!(
            super::INVALID_PARTITIONS,
            ResponseError::InvalidPartitions.code()
        );
        assert_eq!(
            super::INVALID_REPLICATION_FACTOR,
            ResponseError::InvalidReplicationFactor.code()
        );
        assert_eq!(
            super::INVALID_TOPIC_EXCEPTION,
            ResponseError::InvalidTopicException.code()
        );
    }

    #[test]
    fn request_timeout_code_matches_the_dependency() {
        assert_eq!(
            super::REQUEST_TIMED_OUT,
            ResponseError::RequestTimedOut.code()
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

    /// `M4.9`'s own `Heartbeat` code, same oracle.
    #[test]
    fn the_rebalance_in_progress_code_matches_the_dependency() {
        assert_eq!(
            super::REBALANCE_IN_PROGRESS,
            ResponseError::RebalanceInProgress.code()
        );
    }

    /// `M4.11`'s own five fencing codes, same oracle.
    #[test]
    fn the_fencing_codes_match_the_dependency() {
        assert_eq!(
            super::UNKNOWN_MEMBER_ID,
            ResponseError::UnknownMemberId.code()
        );
        assert_eq!(
            super::ILLEGAL_GENERATION,
            ResponseError::IllegalGeneration.code()
        );
        assert_eq!(super::NOT_COORDINATOR, ResponseError::NotCoordinator.code());
        assert_eq!(
            super::COORDINATOR_NOT_AVAILABLE,
            ResponseError::CoordinatorNotAvailable.code()
        );
        assert_eq!(
            super::COORDINATOR_LOAD_IN_PROGRESS,
            ResponseError::CoordinatorLoadInProgress.code()
        );
    }
}
