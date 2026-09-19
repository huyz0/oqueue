//! The bytes of one group-metadata-log segment (`M6.6`): committed offsets
//! and group transitions, in the discipline `metadata_segment.rs` sets —
//! every field explicit, every read bounds-checked, anything not understood
//! whole refused.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use crate::{
    CommitVersion, Error, GroupEvent, GroupId, GroupMetadataEntry, GroupMetadataRecord, Result,
    TopicId,
};

const MAGIC: [u8; 4] = *b"OQGL";
const FORMAT: u8 = 1;
const TAG_OFFSET: u8 = 1;
const TAG_TRANSITION: u8 = 2;

/// Encodes the entries one append carries.
pub(crate) fn encode(entries: &[GroupMetadataEntry]) -> Result<Vec<u8>> {
    let malformed = |at| Error::MalformedMetadataSegment { at };
    let mut out = Vec::with_capacity(48 * entries.len() + 9);
    out.extend_from_slice(&MAGIC);
    out.push(FORMAT);
    let count = u32::try_from(entries.len()).map_err(|_| malformed(out.len()))?;
    out.extend_from_slice(&count.to_be_bytes());
    for entry in entries {
        out.extend_from_slice(&entry.version().get().to_be_bytes());
        match entry.record() {
            GroupMetadataRecord::OffsetCommitted {
                group,
                topic,
                partition,
                offset,
            } => {
                out.push(TAG_OFFSET);
                put_str(&mut out, group.as_str())?;
                put_str(&mut out, topic.as_str())?;
                out.extend_from_slice(&partition.to_be_bytes());
                out.extend_from_slice(&offset.to_be_bytes());
            }
            GroupMetadataRecord::GroupTransitioned { group, event } => {
                out.push(TAG_TRANSITION);
                put_str(&mut out, group.as_str())?;
                out.push(event_tag(*event));
            }
        }
    }
    Ok(out)
}

/// Decodes a segment [`encode`] wrote.
pub(crate) fn decode(bytes: &[u8]) -> Result<Vec<GroupMetadataEntry>> {
    let mut cur = Cursor { bytes, at: 0 };
    if cur.take(4)? != MAGIC || cur.u8()? != FORMAT {
        return Err(cur.malformed());
    }
    let count = u32::from_be_bytes(cur.array()?);
    let mut entries = Vec::new();
    for _ in 0..count {
        let version = CommitVersion::new(u64::from_be_bytes(cur.array()?));
        let at = cur.at;
        let record = match cur.u8()? {
            TAG_OFFSET => {
                let group = cur.group()?;
                let raw = cur.string()?;
                let topic = TopicId::new(raw).map_err(|_| cur.malformed())?;
                let partition = i32::from_be_bytes(cur.array()?);
                let offset = i64::from_be_bytes(cur.array()?);
                GroupMetadataRecord::OffsetCommitted {
                    group,
                    topic,
                    partition,
                    offset,
                }
            }
            TAG_TRANSITION => {
                let group = cur.group()?;
                let event = event_from(cur.u8()?).ok_or_else(|| cur.malformed())?;
                GroupMetadataRecord::GroupTransitioned { group, event }
            }
            _ => return Err(Error::MalformedMetadataSegment { at }),
        };
        entries.push(GroupMetadataEntry::new(version, record));
    }
    if cur.at != bytes.len() {
        return Err(cur.malformed());
    }
    Ok(entries)
}

/// ⚠️ **Explicit, never a cast of the discriminant**: a reordered enum must not
/// silently reinterpret a durable byte.
const fn event_tag(event: GroupEvent) -> u8 {
    match event {
        GroupEvent::Join => 1,
        GroupEvent::JoinBarrierComplete => 2,
        GroupEvent::SyncComplete => 3,
        GroupEvent::MemberJoinedDuringSync => 4,
        GroupEvent::MemberLeft => 5,
        GroupEvent::AllMembersGone => 6,
        GroupEvent::Expire => 7,
    }
}

const fn event_from(tag: u8) -> Option<GroupEvent> {
    Some(match tag {
        1 => GroupEvent::Join,
        2 => GroupEvent::JoinBarrierComplete,
        3 => GroupEvent::SyncComplete,
        4 => GroupEvent::MemberJoinedDuringSync,
        5 => GroupEvent::MemberLeft,
        6 => GroupEvent::AllMembersGone,
        7 => GroupEvent::Expire,
        _ => return None,
    })
}

fn put_str(out: &mut Vec<u8>, value: &str) -> Result<()> {
    let len = u16::try_from(value.len())
        .map_err(|_| Error::MalformedMetadataSegment { at: out.len() })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn malformed(&self) -> Error {
        Error::MalformedMetadataSegment { at: self.at }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(len).ok_or_else(|| self.malformed())?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| self.malformed())?;
        self.at = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.take(N)?;
        slice.try_into().map_err(|_| self.malformed())
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn string(&mut self) -> Result<&'a str> {
        let len = usize::from(u16::from_be_bytes(self.array()?));
        let raw = self.take(len)?;
        core::str::from_utf8(raw).map_err(|_| self.malformed())
    }

    fn group(&mut self) -> Result<GroupId> {
        let raw = self.string()?;
        GroupId::new(raw).map_err(|_| self.malformed())
    }
}

#[cfg(test)]
mod tests;
