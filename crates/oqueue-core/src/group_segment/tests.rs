//! The group segment format round-trips every record and refuses what it
//! cannot read whole.

#![allow(clippy::expect_used)]

use super::{decode, encode};
use crate::{
    CommitVersion, Error, GroupEvent, GroupId, GroupMetadataEntry, GroupMetadataRecord, TopicId,
};

fn every_record() -> Vec<GroupMetadataEntry> {
    let group = GroupId::new("g").expect("a valid group");
    let mut records = vec![GroupMetadataRecord::OffsetCommitted {
        group: group.clone(),
        topic: TopicId::new("orders").expect("a valid topic"),
        partition: 3,
        offset: 42,
    }];
    for event in [
        GroupEvent::Join,
        GroupEvent::JoinBarrierComplete,
        GroupEvent::SyncComplete,
        GroupEvent::MemberJoinedDuringSync,
        GroupEvent::MemberLeft,
        GroupEvent::AllMembersGone,
        GroupEvent::Expire,
    ] {
        records.push(GroupMetadataRecord::GroupTransitioned {
            group: group.clone(),
            event,
        });
    }
    records
        .into_iter()
        .zip(1_u64..)
        .map(|(record, version)| GroupMetadataEntry::new(CommitVersion::new(version), record))
        .collect()
}

#[test]
fn every_record_round_trips() {
    let entries = every_record();
    assert_eq!(
        decode(&encode(&entries).expect("encodes")).expect("decodes"),
        entries
    );
}

#[test]
fn every_truncation_and_trailing_byte_is_refused() {
    let bytes = encode(&every_record()).expect("encodes");
    for len in 0..bytes.len() {
        assert!(
            matches!(
                decode(&bytes[..len]),
                Err(Error::MalformedMetadataSegment { .. })
            ),
            "cut at {len}"
        );
    }
    let mut long = bytes;
    long.push(0);
    assert!(matches!(
        decode(&long),
        Err(Error::MalformedMetadataSegment { .. })
    ));
}

#[test]
fn an_unknown_tag_or_event_is_refused() {
    let entry = GroupMetadataEntry::new(
        CommitVersion::new(1),
        GroupMetadataRecord::GroupTransitioned {
            group: GroupId::new("g").expect("a valid group"),
            event: GroupEvent::Join,
        },
    );
    let good = encode(&[entry]).expect("encodes");
    let mut bad_tag = good.clone();
    bad_tag[17] = 0xee;
    assert_eq!(
        decode(&bad_tag).err(),
        Some(Error::MalformedMetadataSegment { at: 17 })
    );
    let mut bad_event = good;
    let last = bad_event.len() - 1;
    bad_event[last] = 0xee;
    assert!(matches!(
        decode(&bad_event),
        Err(Error::MalformedMetadataSegment { .. })
    ));
}
