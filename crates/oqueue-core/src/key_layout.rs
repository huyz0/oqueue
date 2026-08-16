//! Computable object keys: a pure function from a log position to the
//! [`ObjectKey`] that would hold it — no index lookup, no LIST.

use crate::{ObjectKey, Offset, PartitionId, Result, TopicId};
use std::num::{NonZeroU32, NonZeroU64};

/// FNV-1a, 64-bit.
///
/// ⚠️ **Hand-rolled rather than `std`'s `DefaultHasher`, deliberately.** `std`
/// makes no stability guarantee on `DefaultHasher`'s algorithm across
/// versions, but a key computed today must still compute identically in five
/// years — the whole point of this module is that a reader can derive a key
/// without an index, which only holds if the derivation never changes underfoot.
/// FNV-1a's definition is public, fixed, and small enough that implementing it
/// here costs less than a dependency would.
const fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(PRIME);
        i += 1;
    }
    hash
}

/// The two numbers that decide an [`ObjectKey`]'s shape: how finely offsets
/// are aligned, and how many hash buckets fan out across.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyLayout {
    alignment: NonZeroU64,
    bucket_count: NonZeroU32,
}

impl KeyLayout {
    /// A layout aligning offsets to `alignment` and fanning out across
    /// `bucket_count` hash buckets.
    ///
    /// ⚠️ **`NonZero` rather than a validated constructor.** Zero in either
    /// position has no meaning to reject at a call site — an alignment of
    /// zero aligns nothing, a bucket count of zero has no bucket to compute
    /// — so the type itself makes the invalid values unconstructible, the
    /// same pattern every identifier in this crate already uses.
    #[must_use]
    pub const fn new(alignment: NonZeroU64, bucket_count: NonZeroU32) -> Self {
        Self {
            alignment,
            bucket_count,
        }
    }

    /// The hash bucket `(topic, partition)` fans out to, in `[0, bucket_count)`.
    ///
    /// ⚠️ **Not part of the public key format's guarantee, exposed for
    /// testing distribution.** A caller should not depend on the bucket
    /// number's meaning beyond "some value the key's first path segment
    /// encodes" — only [`KeyLayout::object_key`]'s output is the contract.
    #[must_use]
    fn bucket(&self, topic: &TopicId, partition: PartitionId) -> u32 {
        let hash = fnv1a(format!("{topic}/{partition}").as_bytes());
        // `% bucket_count` fits in `u32` because the divisor does.
        #[allow(clippy::cast_possible_truncation)]
        let bucket = (hash % u64::from(self.bucket_count.get())) as u32;
        bucket
    }

    /// Computes the key for the object that would hold `offset`, in
    /// `topic`'s `partition`.
    ///
    /// Two properties, both load-bearing:
    ///
    /// - **Offset-aligned**: `offset` is floor-aligned to
    ///   [`KeyLayout`]'s alignment, so every offset in the same alignment
    ///   quantum computes the *same* key — the object that would hold it,
    ///   once written. A reader with a target offset and this layout needs
    ///   no index round trip to know where to look.
    /// - **Hash fan-out**: the key's first path segment is a hash bucket
    ///   derived from `(topic, partition)`, spreading one topic-partition's
    ///   whole history across `bucket_count` backend partitions rather than
    ///   one hot prefix (doc 04 §3; doc 16 §5's "offset-aligned object
    ///   naming", the mechanism this one is modelled on).
    ///
    /// ⚠️ **No dependence on lexicographic order.** S3 Express directory
    /// buckets do not return objects in lexicographical order (doc 12 §1.2's
    /// "Express-specific LIST landmine"), so nothing here — or anywhere
    /// downstream — may assume adjacent offsets produce adjacent keys under
    /// string ordering. The zero-padded offset below is for fixed-width
    /// computability, not for sorting.
    ///
    /// # Errors
    ///
    /// [`crate::Error::EmptyObjectKey`] never — the computed string always
    /// contains the bucket number and `topic`, which
    /// [`TopicId`]'s own invariant already guarantees is non-empty. `Result`
    /// only because [`ObjectKey::new`] is the sole way to build one.
    pub fn object_key(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        offset: Offset,
    ) -> Result<ObjectKey> {
        let bucket = self.bucket(topic, partition);
        // `Offset` is never negative (its own invariant), so this cast never
        // truncates or reinterprets sign.
        #[allow(clippy::cast_sign_loss)]
        let raw = offset.get() as u64;
        let aligned = (raw / self.alignment.get()) * self.alignment.get();
        ObjectKey::new(format!("{bucket}/{topic}/{partition}/{aligned:020}"))
    }
}
