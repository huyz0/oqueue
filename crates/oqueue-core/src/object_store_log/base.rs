//! Where a log begins: the base generations a checkpoint commits
//! (`ADR-0046` point 4, `M6.4`).
//!
//! ⚠️ **Split from `object_store_log.rs` at the 500-line limit, along the
//! concept**: that file appends and reads segments; this one is the pointer
//! that says which segments are live.

// ⚠️ `pub(super)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use crate::{ByteRange, Error, ObjectKey, ObjectStore, Result};

/// The bytes a base object begins with.
const BASE_MAGIC: [u8; 4] = *b"OQMB";

/// Where a log begins: its lowest live segment, and the snapshot of
/// everything before it.
pub(super) struct Base {
    pub(super) seq: u64,
    pub(super) snapshot: Option<ObjectKey>,
}

impl Base {
    pub(super) fn encode(&self) -> Result<Vec<u8>> {
        let mut out = BASE_MAGIC.to_vec();
        out.extend_from_slice(&self.seq.to_be_bytes());
        let key = self.snapshot.as_ref().map_or("", ObjectKey::as_str);
        let len =
            u16::try_from(key.len()).map_err(|_| Error::MalformedMetadataSegment { at: 12 })?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(key.as_bytes());
        Ok(out)
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let malformed = |at| Error::MalformedMetadataSegment { at };
        if bytes.get(..4) != Some(&BASE_MAGIC[..]) {
            return Err(malformed(0));
        }
        let seq: [u8; 8] = bytes
            .get(4..12)
            .and_then(|raw| raw.try_into().ok())
            .ok_or_else(|| malformed(4))?;
        let len: [u8; 2] = bytes
            .get(12..14)
            .and_then(|raw| raw.try_into().ok())
            .ok_or_else(|| malformed(12))?;
        let key = bytes
            .get(14..)
            .filter(|rest| rest.len() == usize::from(u16::from_be_bytes(len)))
            .ok_or_else(|| malformed(14))?;
        let key = core::str::from_utf8(key).map_err(|_| malformed(14))?;
        let snapshot = if key.is_empty() {
            None
        } else {
            Some(ObjectKey::new(key).map_err(|_| malformed(14))?)
        };
        Ok(Self {
            seq: u64::from_be_bytes(seq),
            snapshot,
        })
    }
}

pub(super) fn base_key(prefix: &str, generation: u64) -> Result<ObjectKey> {
    ObjectKey::new(format!("{prefix}/base/{generation:020}"))
}

/// Whether base generation `generation` exists, and what it says.
async fn probe(store: &dyn ObjectStore, prefix: &str, generation: u64) -> Result<Option<Base>> {
    match store
        .get(&base_key(prefix, generation)?, ByteRange::Full)
        .await
    {
        Ok(bytes) => Ok(Some(Base::decode(&bytes)?)),
        Err(Error::ObjectNotFound { .. }) => Ok(None),
        Err(other) => Err(other),
    }
}

/// The newest base generation and what it says; generation `None` and
/// segment zero when no checkpoint was ever taken.
///
/// ⚠️ **No LIST, and O(log n) GETs**: generations are written contiguously,
/// each only if absent, so the newest is found by doubling until one is
/// absent and then bisecting between the last present and the first absent.
pub(super) async fn read_base(
    store: &dyn ObjectStore,
    prefix: &str,
) -> Result<(Option<u64>, Base)> {
    let Some(mut found) = probe(store, prefix, 0).await? else {
        return Ok((
            None,
            Base {
                seq: 0,
                snapshot: None,
            },
        ));
    };
    let (mut present, mut absent) = (0_u64, 1_u64);
    while let Some(base) = probe(store, prefix, absent).await? {
        found = base;
        present = absent;
        absent = absent.checked_mul(2).ok_or(Error::CommitVersionOverflow {
            base: absent,
            delta: absent,
        })?;
    }
    while absent - present > 1 {
        let middle = present + (absent - present) / 2;
        match probe(store, prefix, middle).await? {
            Some(base) => {
                found = base;
                present = middle;
            }
            None => absent = middle,
        }
    }
    Ok((Some(present), found))
}
