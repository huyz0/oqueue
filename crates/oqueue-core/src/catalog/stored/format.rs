//! Durable catalog entry encoding and decoding.

use super::{
    CUSTOMER_FORMAT, CatalogEntry, DEFAULT_FORMAT, OWNER_CUSTOMER_FORMAT, OWNER_DEFAULT_FORMAT,
};
use crate::{Error, KeyDomain, Principal, Result, TopicId};
use std::fmt::Write as _;

/// The default format remains byte-identical: `DEFAULT_FORMAT`, partition
/// count big-endian, then the name's bytes. Customer entries use a v2 shape
/// with explicit name and key-id lengths so both metadata fields can be read
/// back without guessing a boundary. Formats 3 and 4 add an explicit creator
/// length and bytes before the existing name/key fields.
pub(super) fn encode(entry: &CatalogEntry) -> Result<Vec<u8>> {
    match (entry.key_domain(), entry.creator()) {
        (KeyDomain::Default, None) => Ok(encode_default(entry)),
        (KeyDomain::Default, Some(creator)) => encode_owned_default(entry, creator),
        (KeyDomain::Customer(key), None) => encode_customer(entry, key.as_str().as_bytes()),
        (KeyDomain::Customer(key), Some(creator)) => {
            encode_owned_customer(entry, key.as_str().as_bytes(), creator.as_str().as_bytes())
        }
    }
}

fn encode_default(entry: &CatalogEntry) -> Vec<u8> {
    let mut out = vec![DEFAULT_FORMAT];
    out.extend_from_slice(&entry.partitions().to_be_bytes());
    out.extend_from_slice(entry.name().as_str().as_bytes());
    out
}

fn encode_owned_default(entry: &CatalogEntry, creator: &Principal) -> Result<Vec<u8>> {
    let creator = creator.as_str().as_bytes();
    let creator_len =
        u16::try_from(creator.len()).map_err(|_| Error::MalformedMetadataSegment { at: 5 })?;
    let mut out = vec![OWNER_DEFAULT_FORMAT];
    out.extend_from_slice(&entry.partitions().to_be_bytes());
    out.extend_from_slice(&creator_len.to_be_bytes());
    out.extend_from_slice(creator);
    out.extend_from_slice(entry.name().as_str().as_bytes());
    Ok(out)
}

fn encode_customer(entry: &CatalogEntry, key: &[u8]) -> Result<Vec<u8>> {
    let name = entry.name().as_str().as_bytes();
    let name_len =
        u16::try_from(name.len()).map_err(|_| Error::MalformedMetadataSegment { at: 5 })?;
    let key_len = u16::try_from(key.len())
        .map_err(|_| Error::MalformedMetadataSegment { at: 7 + name.len() })?;
    let mut out = vec![CUSTOMER_FORMAT];
    out.extend_from_slice(&entry.partitions().to_be_bytes());
    out.extend_from_slice(&name_len.to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&key_len.to_be_bytes());
    out.extend_from_slice(key);
    Ok(out)
}

fn encode_owned_customer(entry: &CatalogEntry, key: &[u8], creator: &[u8]) -> Result<Vec<u8>> {
    let name = entry.name().as_str().as_bytes();
    let name_len =
        u16::try_from(name.len()).map_err(|_| Error::MalformedMetadataSegment { at: 5 })?;
    let key_len = u16::try_from(key.len())
        .map_err(|_| Error::MalformedMetadataSegment { at: 7 + name.len() })?;
    let creator_len =
        u16::try_from(creator.len()).map_err(|_| Error::MalformedMetadataSegment {
            at: 9 + name.len() + key.len(),
        })?;
    let mut out = vec![OWNER_CUSTOMER_FORMAT];
    out.extend_from_slice(&entry.partitions().to_be_bytes());
    out.extend_from_slice(&name_len.to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&key_len.to_be_bytes());
    out.extend_from_slice(key);
    out.extend_from_slice(&creator_len.to_be_bytes());
    out.extend_from_slice(creator);
    Ok(out)
}

/// ⚠️ **Refused whole, never misread**: an unknown format, a short body, or a
/// name that is not a topic's is [`Error::MalformedMetadataSegment`].
pub(super) fn decode(bytes: &[u8]) -> Result<CatalogEntry> {
    let format = *bytes
        .first()
        .ok_or(Error::MalformedMetadataSegment { at: 0 })?;
    let partitions: [u8; 4] = bytes
        .get(1..5)
        .and_then(|raw| raw.try_into().ok())
        .ok_or(Error::MalformedMetadataSegment { at: 1 })?;
    let partitions = u32::from_be_bytes(partitions);
    match format {
        DEFAULT_FORMAT => decode_default(bytes, partitions),
        CUSTOMER_FORMAT => decode_customer(bytes, partitions),
        OWNER_DEFAULT_FORMAT => decode_owner_default(bytes, partitions),
        OWNER_CUSTOMER_FORMAT => decode_owner_customer(bytes, partitions),
        _ => Err(Error::MalformedMetadataSegment { at: 0 }),
    }
}

fn decode_default(bytes: &[u8], partitions: u32) -> Result<CatalogEntry> {
    let name = bytes
        .get(5..)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|name| TopicId::new(name).ok())
        .ok_or(Error::MalformedMetadataSegment { at: 5 })?;
    Ok(CatalogEntry::new(name, partitions))
}

fn decode_customer(bytes: &[u8], partitions: u32) -> Result<CatalogEntry> {
    let name_len = bytes
        .get(5..7)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: 5 })?;
    let name_len = usize::from(name_len);
    let name_end = 7usize
        .checked_add(name_len)
        .ok_or(Error::MalformedMetadataSegment { at: 7 })?;
    let name = bytes
        .get(7..name_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|name| TopicId::new(name).ok())
        .ok_or(Error::MalformedMetadataSegment { at: 7 })?;
    let key_len_at = name_end;
    let key_len = bytes
        .get(key_len_at..key_len_at + 2)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: key_len_at })?;
    let key_start = key_len_at + 2;
    let key_end = key_start
        .checked_add(usize::from(key_len))
        .ok_or(Error::MalformedMetadataSegment { at: key_start })?;
    let key = bytes
        .get(key_start..key_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|key| crate::KeyId::new(key).ok())
        .ok_or(Error::MalformedMetadataSegment { at: key_start })?;
    if key_end != bytes.len() {
        return Err(Error::MalformedMetadataSegment { at: key_end });
    }
    Ok(CatalogEntry::with_key_domain(
        name,
        partitions,
        KeyDomain::customer(key),
    ))
}

fn decode_owner_default(bytes: &[u8], partitions: u32) -> Result<CatalogEntry> {
    let creator_len = bytes
        .get(5..7)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: 5 })?;
    let creator_start = 7;
    let creator_end = creator_start + usize::from(creator_len);
    let creator = bytes
        .get(creator_start..creator_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|text| Principal::new(text).ok())
        .ok_or(Error::MalformedMetadataSegment { at: creator_start })?;
    let name = bytes
        .get(creator_end..)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|text| TopicId::new(text).ok())
        .ok_or(Error::MalformedMetadataSegment { at: creator_end })?;
    Ok(CatalogEntry::with_creator(name, partitions, creator))
}

fn decode_owner_customer(bytes: &[u8], partitions: u32) -> Result<CatalogEntry> {
    let entry = decode_customer_prefix(bytes, partitions)?;
    let key_end = entry.0;
    let creator_len = bytes
        .get(key_end..key_end + 2)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: key_end })?;
    let creator_start = key_end + 2;
    let creator_end = creator_start + usize::from(creator_len);
    if creator_end != bytes.len() {
        return Err(Error::MalformedMetadataSegment { at: creator_end });
    }
    let creator = bytes
        .get(creator_start..creator_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|text| Principal::new(text).ok())
        .ok_or(Error::MalformedMetadataSegment { at: creator_start })?;
    Ok(CatalogEntry::with_key_domain_and_creator(
        entry.1, partitions, entry.2, creator,
    ))
}

fn decode_customer_prefix(bytes: &[u8], _partitions: u32) -> Result<(usize, TopicId, KeyDomain)> {
    let name_len = bytes
        .get(5..7)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: 5 })?;
    let name_start = 7;
    let name_end = name_start + usize::from(name_len);
    let name = bytes
        .get(name_start..name_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|text| TopicId::new(text).ok())
        .ok_or(Error::MalformedMetadataSegment { at: name_start })?;
    let key_len_at = name_end;
    let key_len = bytes
        .get(key_len_at..key_len_at + 2)
        .and_then(|raw| raw.try_into().ok())
        .map(u16::from_be_bytes)
        .ok_or(Error::MalformedMetadataSegment { at: key_len_at })?;
    let key_start = key_len_at + 2;
    let key_end = key_start + usize::from(key_len);
    let key = bytes
        .get(key_start..key_end)
        .and_then(|raw| String::from_utf8(raw.to_vec()).ok())
        .and_then(|text| crate::KeyId::new(text).ok())
        .ok_or(Error::MalformedMetadataSegment { at: key_start })?;
    Ok((key_end, name, KeyDomain::customer(key)))
}

pub(super) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// ⚠️ Lowercase only, as [`hex`] writes: anything else is not one of ours.
pub(super) fn unhex(text: &str) -> Option<Vec<u8>> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let raw = text.as_bytes();
    if !raw.len().is_multiple_of(2) {
        return None;
    }
    raw.chunks_exact(2)
        .map(|pair| match pair {
            [high, low] => Some(digit(*high)? << 4 | digit(*low)?),
            _ => None,
        })
        .collect()
}
