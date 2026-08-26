//! Naming a bundled object so that no two flushes ever collide.
//!
//! ⚠️ **Its own module because naming is not the format**, and because
//! `bundle.rs` reached the 500-line limit. What an object *contains* and what
//! it is *called* answer different questions, and `ADR-0026`'s decision to
//! write without a `Precondition` rests entirely on the second.

use crate::{Error, ObjectKey, Result};

/// Names bundled objects so that no two flushes ever collide.
///
/// # ⚠️ Why this exists, and what it rests on
///
/// `ADR-0026` decided `M3.13`'s flush writes with no `Precondition` — the
/// offset stream must not use conditional writes (`ADR-0020` point 4), and a
/// retried flush is safe because it rewrites its own key. **That is only true
/// if the key is one nothing else writes**, and the repository does not supply
/// such a key: [`KeyLayout::object_key`](crate::KeyLayout::object_key)
/// floor-aligns an offset to a quantum, so every offset in one quantum computes
/// the same name, and it takes a single `(topic, partition)`, which an object
/// spanning N topics is not.
///
/// So this is the naming, in doc 12 §4.6's shape — a monotonic sequence per
/// writer. ⚠️ **Collision-freedom rests on writer identities being distinct,
/// and that is the composer's obligation, not this type's.** It is stated here
/// rather than assumed because it is the whole of what makes the unconditional
/// `put` safe: two live brokers sharing an identity would overwrite each
/// other's objects, and the records lost would be ones already acknowledged.
/// ⚠️ **Nothing mints one yet**, and this sentence used to say `bin/oqueue`
/// did. It is the composer at `M3.14` that must, per *process* — a node id or
/// a configuration value would be shared by two brokers on one host, and by a
/// restart, and the second writer's first flush would overwrite the first's
/// object at sequence zero.
///
/// ⚠️ **A restart must not reuse identities either.** A process that came back
/// with the same identity and a sequence restarted at zero would rewrite its
/// predecessor's objects — which is why the identity is per *process*, not per
/// node or per configuration.
#[derive(Debug)]
pub struct BundleNamer {
    writer: String,
    next: u64,
}

impl BundleNamer {
    /// A namer for one writer.
    ///
    /// # Errors
    ///
    /// [`Error::EmptyObjectKey`] if `writer` is empty, and
    /// [`Error::MalformedWriterId`] if it contains a `/` — which would let two
    /// distinct identities produce one key by moving the boundary between the
    /// writer and its sequence.
    pub fn new(writer: impl Into<String>) -> Result<Self> {
        let writer = writer.into();
        if writer.is_empty() {
            return Err(Error::EmptyObjectKey);
        }
        if writer.contains('/') {
            return Err(Error::MalformedWriterId);
        }
        Ok(Self { writer, next: 0 })
    }

    /// The key for the next object this writer flushes.
    ///
    /// ⚠️ Zero-padded so the sequence is fixed-width, and **not** so it sorts:
    /// S3 Express directory buckets do not return objects in lexicographic
    /// order (doc 12 §1.2), so nothing may depend on adjacent keys being
    /// adjacent under string ordering.
    ///
    /// # Errors
    ///
    /// [`Error::BundleSequenceExhausted`] if this writer has flushed
    /// `u64::MAX` objects. ⚠️ An error rather than a wrap, for the reason
    /// [`Offset::add`](crate::Offset::add) refuses to wrap: the next name after
    /// a wrap is one already used, and the write that follows overwrites
    /// acknowledged records.
    pub fn next_key(&mut self) -> Result<ObjectKey> {
        let sequence = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or(Error::BundleSequenceExhausted)?;
        ObjectKey::new(format!("bundles/{}/{sequence:020}", self.writer))
    }
}
