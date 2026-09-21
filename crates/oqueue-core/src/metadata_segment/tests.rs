//! The segment format round-trips every record, and refuses what it cannot
//! read whole.

#![allow(clippy::expect_used)]

use super::{decode_segment, encode_segment};
use crate::{
    ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error, MetadataEntry,
    MetadataRecord, ObjectKey, ObjectRef, Offset, PartitionId, ProducerEpoch, ProducerId,
    ProducerIdentity, Timestamp, TopicId,
};

fn topic() -> TopicId {
    TopicId::new("orders").expect("a valid topic")
}

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a valid key")
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

/// One entry of every record kind, with every optional field both ways.
fn every_record() -> Vec<MetadataEntry> {
    let partition = PartitionId::new(3).expect("a valid partition");
    let records = vec![
        batch_record(partition),
        MetadataRecord::ManifestPublished {
            topic: topic(),
            partition,
            manifest: key("manifest-1"),
            upto: offset(7),
        },
        compacted_record(partition),
        MetadataRecord::Trimmed {
            topic: topic(),
            partition,
            start: offset(4),
        },
        MetadataRecord::EpochChanged {
            epoch: CoordinatorEpoch::new(9),
        },
        MetadataRecord::TopicRetentionChanged {
            topic: topic(),
            retention_ms: Some(900_000),
        },
    ];
    records
        .into_iter()
        .zip(10_u64..)
        .map(|(record, version)| MetadataEntry::new(CommitVersion::new(version), record))
        .collect()
}

fn batch_record(partition: PartitionId) -> MetadataRecord {
    let producer = ProducerIdentity::new(
        ProducerId::new(42).expect("a valid id"),
        ProducerEpoch::new(7).expect("a valid epoch"),
        99,
    );
    MetadataRecord::BatchCommitted {
        object: key("obj-1"),
        spans: vec![
            CommittedSpan::new(topic(), partition, 5, ByteRange::Full, None),
            CommittedSpan::new(
                topic(),
                partition,
                2,
                ByteRange::bounded(10, 20).expect("a range"),
                Some(producer),
            ),
        ],
        written_at: Timestamp::from_millis(1_700_000_000_000).expect("a time"),
    }
}

fn compacted_record(partition: PartitionId) -> MetadataRecord {
    MetadataRecord::RangeCompacted {
        topic: topic(),
        partition,
        retiring: vec![
            ObjectRef::new(key("a"), offset(0), 3),
            ObjectRef::new(key("b"), offset(3), 4),
        ],
        installing: vec![ObjectRef::new(key("merged"), offset(0), 7)],
    }
}

#[test]
fn every_record_round_trips() {
    let entries = every_record();
    let bytes = encode_segment(&entries).expect("encodes");
    assert_eq!(decode_segment(&bytes).expect("decodes"), entries);
}

#[test]
fn an_empty_segment_round_trips() {
    let bytes = encode_segment(&[]).expect("encodes");
    assert!(decode_segment(&bytes).expect("decodes").is_empty());
}

/// ⚠️ **Every truncation is refused**, never read as a shorter log: a segment
/// cut anywhere names where the parser stopped.
#[test]
fn every_truncation_is_refused() {
    let bytes = encode_segment(&every_record()).expect("encodes");
    for len in 0..bytes.len() {
        assert!(
            matches!(
                decode_segment(&bytes[..len]),
                Err(Error::MalformedMetadataSegment { .. })
            ),
            "a segment cut at {len} of {} decoded",
            bytes.len()
        );
    }
}

#[test]
fn trailing_bytes_are_refused() {
    let mut bytes = encode_segment(&every_record()).expect("encodes");
    bytes.push(0);
    assert!(matches!(
        decode_segment(&bytes),
        Err(Error::MalformedMetadataSegment { .. })
    ));
}

#[test]
fn a_wrong_magic_or_format_is_refused() {
    let good = encode_segment(&every_record()).expect("encodes");
    for at in [0, 4] {
        let mut bad = good.clone();
        bad[at] ^= 0xff;
        assert!(
            matches!(
                decode_segment(&bad),
                Err(Error::MalformedMetadataSegment { at: 0 | 4 | 5 })
            ),
            "a flipped byte {at} decoded"
        );
    }
}

#[test]
fn an_unknown_record_tag_is_refused_where_it_sits() {
    let entry = MetadataEntry::new(
        CommitVersion::new(1),
        MetadataRecord::EpochChanged {
            epoch: CoordinatorEpoch::new(1),
        },
    );
    let mut bytes = encode_segment(&[entry]).expect("encodes");
    // magic 4 + format 1 + count 4 + version 8 = the tag's position.
    bytes[17] = 0xee;
    assert_eq!(
        decode_segment(&bytes).err(),
        Some(Error::MalformedMetadataSegment { at: 17 })
    );
}

/// A field no constructor accepts — here an empty byte range — is refused,
/// not built around.
#[test]
fn a_field_no_constructor_accepts_is_refused() {
    let entry = MetadataEntry::new(
        CommitVersion::new(1),
        MetadataRecord::BatchCommitted {
            object: key("o"),
            spans: vec![CommittedSpan::new(
                topic(),
                PartitionId::new(0).expect("a valid partition"),
                1,
                ByteRange::bounded(0, 1).expect("a range"),
                None,
            )],
            written_at: Timestamp::EPOCH,
        },
    );
    let mut bytes = encode_segment(&[entry]).expect("encodes");
    // The bounded length is the last 8 bytes before the producer flag.
    let len_at = bytes.len() - 1 - 8;
    bytes[len_at..len_at + 8].copy_from_slice(&0_u64.to_be_bytes());
    assert!(matches!(
        decode_segment(&bytes),
        Err(Error::MalformedMetadataSegment { .. })
    ));
}
