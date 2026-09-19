//! The bytes of one metadata-log segment: the entries one `append` carried
//! (`ADR-0046`, `M6.1`).
//!
//! ⚠️ **A durable format, read back by a process that did not write it.** A
//! segment outlives the coordinator that wrote it, so every field is written
//! explicitly and big-endian, and the decoder refuses anything it does not
//! fully understand rather than guessing: a log that folds a half-read record
//! mis-bases every later offset, which is worse than a log that will not open.
//!
//! ⚠️ **The decoder never panics and never allocates from a length it has not
//! checked against the bytes present.** Object storage is shared, and a
//! segment is input this process did not produce in this lifetime.

use crate::{
    ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error, MetadataEntry,
    MetadataRecord, ObjectKey, ObjectRef, Offset, PartitionId, ProducerEpoch, ProducerId,
    ProducerIdentity, Result, Timestamp, TopicId,
};

/// The bytes a segment begins with.
const MAGIC: [u8; 4] = *b"OQML";
/// The format version this build writes and reads.
const FORMAT: u8 = 1;

const TAG_BATCH: u8 = 1;
const TAG_MANIFEST: u8 = 2;
const TAG_COMPACTED: u8 = 3;
const TAG_TRIMMED: u8 = 4;
const TAG_EPOCH: u8 = 5;

/// Encodes the entries one append carries.
///
/// # Errors
///
/// [`Error::MalformedMetadataSegment`] if a string or a list is longer than
/// its length field can carry — a record this format cannot represent is
/// refused before anything is written, never truncated.
pub fn encode_segment(entries: &[MetadataEntry]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(64 * entries.len() + 9);
    out.extend_from_slice(&MAGIC);
    out.push(FORMAT);
    put_len32(&mut out, entries.len())?;
    for entry in entries {
        out.extend_from_slice(&entry.version().get().to_be_bytes());
        encode_record(entry.record(), &mut out)?;
    }
    Ok(out)
}

/// Decodes a segment [`encode_segment`] wrote.
///
/// # Errors
///
/// [`Error::MalformedMetadataSegment`] naming how far in the parser got, for
/// a wrong magic or version, a truncated or overlong segment, an unknown
/// record tag, or a field no constructor accepts.
pub fn decode_segment(bytes: &[u8]) -> Result<Vec<MetadataEntry>> {
    let mut cur = Cursor { bytes, at: 0 };
    if cur.take(4)? != MAGIC || cur.u8()? != FORMAT {
        return Err(cur.malformed());
    }
    let count = cur.u32()?;
    let mut entries = Vec::new();
    for _ in 0..count {
        let version = CommitVersion::new(cur.u64()?);
        let record = decode_record(&mut cur)?;
        entries.push(MetadataEntry::new(version, record));
    }
    if cur.at != bytes.len() {
        return Err(cur.malformed());
    }
    Ok(entries)
}

fn encode_record(record: &MetadataRecord, out: &mut Vec<u8>) -> Result<()> {
    match record {
        MetadataRecord::BatchCommitted {
            object,
            spans,
            written_at,
        } => encode_batch(object, spans, *written_at, out)?,
        MetadataRecord::ManifestPublished {
            topic,
            partition,
            manifest,
            upto,
        } => {
            out.push(TAG_MANIFEST);
            put_partition(out, topic, *partition)?;
            put_str(out, manifest.as_str())?;
            out.extend_from_slice(&upto.get().to_be_bytes());
        }
        MetadataRecord::RangeCompacted {
            topic,
            partition,
            retiring,
            installing,
        } => {
            out.push(TAG_COMPACTED);
            put_partition(out, topic, *partition)?;
            put_refs(out, retiring)?;
            put_refs(out, installing)?;
        }
        MetadataRecord::Trimmed {
            topic,
            partition,
            start,
        } => {
            out.push(TAG_TRIMMED);
            put_partition(out, topic, *partition)?;
            out.extend_from_slice(&start.get().to_be_bytes());
        }
        MetadataRecord::EpochChanged { epoch } => {
            out.push(TAG_EPOCH);
            out.extend_from_slice(&epoch.get().to_be_bytes());
        }
    }
    Ok(())
}

fn encode_batch(
    object: &ObjectKey,
    spans: &[CommittedSpan],
    written_at: Timestamp,
    out: &mut Vec<u8>,
) -> Result<()> {
    out.push(TAG_BATCH);
    put_str(out, object.as_str())?;
    out.extend_from_slice(&written_at.as_millis().to_be_bytes());
    put_len32(out, spans.len())?;
    for span in spans {
        encode_span(span, out)?;
    }
    Ok(())
}

fn decode_record(cur: &mut Cursor<'_>) -> Result<MetadataRecord> {
    let at = cur.at;
    Ok(match cur.u8()? {
        TAG_BATCH => decode_batch(cur)?,
        TAG_MANIFEST => {
            let (topic, partition) = cur.partition()?;
            let manifest = cur.key()?;
            let upto = cur.offset()?;
            MetadataRecord::ManifestPublished {
                topic,
                partition,
                manifest,
                upto,
            }
        }
        TAG_COMPACTED => {
            let (topic, partition) = cur.partition()?;
            let retiring = cur.refs()?;
            let installing = cur.refs()?;
            MetadataRecord::RangeCompacted {
                topic,
                partition,
                retiring,
                installing,
            }
        }
        TAG_TRIMMED => {
            let (topic, partition) = cur.partition()?;
            let start = cur.offset()?;
            MetadataRecord::Trimmed {
                topic,
                partition,
                start,
            }
        }
        TAG_EPOCH => MetadataRecord::EpochChanged {
            epoch: CoordinatorEpoch::new(cur.u64()?),
        },
        _ => return Err(Error::MalformedMetadataSegment { at }),
    })
}

fn decode_batch(cur: &mut Cursor<'_>) -> Result<MetadataRecord> {
    let object = cur.key()?;
    let millis = cur_i64(cur)?;
    let written_at = cur.checked(Timestamp::from_millis(millis))?;
    let count = cur.u32()?;
    let mut spans = Vec::new();
    for _ in 0..count {
        spans.push(decode_span(cur)?);
    }
    Ok(MetadataRecord::BatchCommitted {
        object,
        spans,
        written_at,
    })
}

fn encode_span(span: &CommittedSpan, out: &mut Vec<u8>) -> Result<()> {
    put_partition(out, span.topic(), span.partition())?;
    out.extend_from_slice(&span.record_count().to_be_bytes());
    match span.bytes() {
        ByteRange::Full => out.push(0),
        ByteRange::Bounded(bounded) => {
            out.push(1);
            out.extend_from_slice(&bounded.offset().to_be_bytes());
            out.extend_from_slice(&bounded.length().to_be_bytes());
        }
    }
    match span.producer() {
        None => out.push(0),
        Some(producer) => {
            out.push(1);
            out.extend_from_slice(&producer.id().get().to_be_bytes());
            out.extend_from_slice(&producer.epoch().get().to_be_bytes());
            out.extend_from_slice(&producer.sequence().to_be_bytes());
        }
    }
    Ok(())
}

fn decode_span(cur: &mut Cursor<'_>) -> Result<CommittedSpan> {
    let (topic, partition) = cur.partition()?;
    let records = cur.u32()?;
    let bytes = match cur.u8()? {
        0 => ByteRange::Full,
        1 => {
            let offset = cur.u64()?;
            let length = cur.u64()?;
            cur.checked(ByteRange::bounded(offset, length))?
        }
        _ => return Err(cur.malformed()),
    };
    let producer = match cur.u8()? {
        0 => None,
        1 => {
            let raw_id = cur_i64(cur)?;
            let id = cur.checked(ProducerId::new(raw_id))?;
            let raw_epoch = i16::from_be_bytes(cur.array()?);
            let epoch = cur.checked(ProducerEpoch::new(raw_epoch))?;
            let sequence = i32::from_be_bytes(cur.array()?);
            Some(ProducerIdentity::new(id, epoch, sequence))
        }
        _ => return Err(cur.malformed()),
    };
    Ok(CommittedSpan::new(
        topic, partition, records, bytes, producer,
    ))
}

fn put_len32(out: &mut Vec<u8>, len: usize) -> Result<()> {
    let len = u32::try_from(len).map_err(|_| Error::MalformedMetadataSegment { at: out.len() })?;
    out.extend_from_slice(&len.to_be_bytes());
    Ok(())
}

fn put_str(out: &mut Vec<u8>, value: &str) -> Result<()> {
    let len = u16::try_from(value.len())
        .map_err(|_| Error::MalformedMetadataSegment { at: out.len() })?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn put_partition(out: &mut Vec<u8>, topic: &TopicId, partition: PartitionId) -> Result<()> {
    put_str(out, topic.as_str())?;
    out.extend_from_slice(&partition.get().to_be_bytes());
    Ok(())
}

fn put_refs(out: &mut Vec<u8>, refs: &[ObjectRef]) -> Result<()> {
    put_len32(out, refs.len())?;
    for reference in refs {
        put_str(out, reference.object().as_str())?;
        out.extend_from_slice(&reference.base_offset().get().to_be_bytes());
        out.extend_from_slice(&reference.record_count().to_be_bytes());
    }
    Ok(())
}

fn cur_i64(cur: &mut Cursor<'_>) -> Result<i64> {
    Ok(i64::from_be_bytes(cur.array()?))
}

/// A read position over a segment's bytes, every read bounds-checked.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn malformed(&self) -> Error {
        Error::MalformedMetadataSegment { at: self.at }
    }

    /// A constructor's refusal, reported as a malformed segment here.
    fn checked<T>(&self, value: Result<T>) -> Result<T> {
        value.map_err(|_| self.malformed())
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

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn string(&mut self) -> Result<&'a str> {
        let len = usize::from(u16::from_be_bytes(self.array()?));
        let raw = self.take(len)?;
        core::str::from_utf8(raw).map_err(|_| self.malformed())
    }

    fn key(&mut self) -> Result<ObjectKey> {
        let raw = self.string()?;
        self.checked(ObjectKey::new(raw))
    }

    fn offset(&mut self) -> Result<Offset> {
        let raw = cur_i64(self)?;
        self.checked(Offset::new(raw))
    }

    fn partition(&mut self) -> Result<(TopicId, PartitionId)> {
        let raw = self.string()?;
        let topic = self.checked(TopicId::new(raw))?;
        let index = i32::from_be_bytes(self.array()?);
        let partition = self.checked(PartitionId::new(index))?;
        Ok((topic, partition))
    }

    fn refs(&mut self) -> Result<Vec<ObjectRef>> {
        let count = self.u32()?;
        let mut refs = Vec::new();
        for _ in 0..count {
            let object = self.key()?;
            let base = self.offset()?;
            let records = self.u32()?;
            refs.push(ObjectRef::new(object, base, records));
        }
        Ok(refs)
    }
}

#[cfg(test)]
mod tests;
