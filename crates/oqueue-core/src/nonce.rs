//! The AES-GCM nonce, as a type no call sequence can make repeat.
//!
//! `ADR-0050` point 2: *writer epoch ‖ object sequence ‖ region index, with no
//! constructor able to produce a duplicate and no way to build one from raw
//! bytes.* ⚠️ **This is the one catastrophic mistake available in `M8`.** A
//! nonce repeated under one AES-GCM key does not merely weaken the ciphertext:
//! it leaks the XOR of the two plaintexts and, worse, exposes the GHASH
//! authentication subkey, which lets an attacker forge arbitrary messages
//! under that key. Every other failure in the milestone degrades something; a
//! repeat here removes both confidentiality and integrity at once.
//!
//! So the whole design here is negative space — what a caller *cannot* write:
//!
//! - there is no `Nonce::from_bytes`, no `From<[u8; 12]>`, no `Default`, and
//!   no field a caller can set. A [`Nonce`] — the thing a *seal* takes —
//!   exists only as the return value of [`NonceSource::for_region`]. Bytes
//!   read back out of a footer decode to a [`ParsedNonce`], a **different
//!   type** with no path to a `Nonce`, so a decoded value cannot be fed to a
//!   seal even by mistake;
//! - [`NonceSource`] is **not `Clone` and not `Copy`**, so there is no
//!   clone-and-mutate path that would rewind a counter and hand out a triple
//!   already used;
//! - the object sequence is **not an argument**. Only [`NonceMinter`] — one
//!   per writer epoch, also not `Clone` — can make a [`NonceSource`], and it
//!   hands out each object sequence exactly once from a counter that only
//!   advances. A caller cannot pick one, so a caller cannot repeat one;
//! - the region counter only advances, and [`NonceSource::for_region`] refuses
//!   any index that is not the next one, so calling it twice for one region is
//!   an error rather than a repeat.
//!
//! # ⚠️ `Debug` prints the bytes, deliberately — do not "fix" this
//!
//! A GCM nonce is **public**: it is written beside the ciphertext in the
//! region header and handed to every reader. It is not key material, so it is
//! not wrapped in [`Redacted`](crate::Redacted) the way [`Dek`](crate::Dek)
//! and [`WrappedKey`](crate::WrappedKey) are, and a later reader tidying this
//! module for consistency with `key.rs` would be removing the one thing that
//! makes a nonce-reuse bug *visible* in a log. FR-44 is about secrets; this is
//! not one. What must never be logged is the DEK, and that type already
//! refuses to be.
//!
//! # The obligation this type cannot discharge
//!
//! ⚠️ **Uniqueness rests on writer epochs being distinct, and that is the
//! composer's obligation, not this type's** — the same sentence
//! [`BundleNamer`](crate::BundleNamer) already carries about writer
//! identities, for the same reason and with the same consequence if it is
//! broken. This module makes the *triple* injective into 96 bits; it cannot
//! know whether two live brokers were handed the same epoch.
//!
//! ⚠️ **The writer epoch comes from the coordinator fence, not `WriterId`.**
//! `WriterId::mint` (in `oqueue-broker`) produces a *string* —
//! `{pid:x}-{nanos:x}-{nth:x}` — and is suitable for object naming, but its
//! timestamp is not a durable restart fence. [`WriterEpoch`] therefore has no
//! raw constructor: the composer derives it from [`CoordinatorEpoch`], whose
//! value changes with the fenced coordinator incarnation.
//!
//! **What must hold for uniqueness:**
//!
//! 1. no two writer incarnations that could ever seal under the same DEK are
//!    given the same coordinator-fenced `writer_epoch` — including a process
//!    and its own restart. [`WriterEpoch`] makes the source explicit, while
//!    the coordinator remains responsible for advancing its fence on restart;
//!    `a_restarted_writer_uses_a_new_coordinator_epoch` pins that contract and
//!    `a_restarted_writer_reusing_its_epoch_repeats_nonces` documents the
//!    refusal boundary if a composer violates it;
//! 2. within one writer epoch, `object_sequence` never repeats — ⚠️ **no
//!    longer an obligation**: [`NonceMinter`] owns the counter and there is
//!    no public way to name an object sequence at all;
//! 3. within one object, each region is sealed at most once — also enforced,
//!    by [`NonceSource`]'s own advancing counter.
//!
//! Given (1), the layout below makes (2) and (3) hold by construction.

use crate::{CoordinatorEpoch, Error, Result};

/// A GCM nonce is 96 bits — 12 bytes.
///
/// ⚠️ Fixed by AES-GCM, not chosen: 96 bits is the only width for which the
/// standard uses the nonce directly rather than hashing it, and any other
/// width is a different construction rather than a weaker setting of this one.
pub const NONCE_BYTES: usize = 12;

/// The largest writer epoch the layout can express.
pub const MAX_WRITER_EPOCH: u64 = (1 << 40) - 1;

/// The largest object sequence the layout can express.
pub const MAX_OBJECT_SEQUENCE: u64 = (1 << 40) - 1;

/// The largest region index the layout can express.
pub const MAX_REGION_INDEX: u32 = (1 << 16) - 1;

/// A writer epoch derived from the coordinator's fenced incarnation.
///
/// A writer must not choose this value from a process id, a wall-clock stamp,
/// or configuration: each can repeat after a restart. The coordinator epoch
/// is the durable fence that changes when leadership changes, so it is the
/// only public source for a production writer epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WriterEpoch(u64);

impl WriterEpoch {
    /// Derives the nonce writer epoch from the coordinator's fence.
    #[must_use]
    pub const fn from_coordinator_epoch(epoch: CoordinatorEpoch) -> Self {
        Self(epoch.get())
    }

    /// The encoded epoch value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A 96-bit AES-GCM nonce.
///
/// ⚠️ **There is no way to build one from bytes.** The only constructor is
/// [`NonceSource::for_region`], which is what makes "no call sequence repeats a
/// nonce" a property of the type rather than of every call site. Decoding a
/// footer's twelve bytes gives a [`ParsedNonce`] instead — see that type for
/// why the two are not one type with a second constructor.
///
/// # The bit layout, and why it is injective
///
/// Twelve bytes, big-endian, three fixed-width fields with no overlap:
///
/// ```text
/// byte:  0   1   2   3   4   5   6   7   8   9  10  11
///      +---+---+---+---+---+---+---+---+---+---+---+---+
///      |   writer epoch    |  object sequence  | region|
///      |     (40 bits)     |     (40 bits)     |(16 b) |
///      +---+---+---+---+---+---+---+---+---+---+---+---+
/// ```
///
/// Because every field has a **fixed** width and every input is refused unless
/// it fits that width, the map from `(writer_epoch, object_sequence,
/// region_index)` to twelve bytes is injective: recovering the triple is just
/// slicing at byte 5 and byte 10, so two distinct triples cannot produce one
/// nonce. ⚠️ **Out-of-range inputs are refused, never truncated** — a truncated
/// field is precisely how two distinct triples would collide, and it would do
/// so silently.
///
/// 40/40/16 rather than some other split is a ranging decision: 2^40 writer
/// incarnations and 2^40 objects per incarnation are both far beyond what
/// `ADR-0050` point 3's 64 GiB-per-DEK rotation bound can reach, and 65 536
/// regions is far beyond the `(topic, partition)` count one flush bundles.
/// ⚠️ **Neither `Clone` nor `Copy`, deliberately** (`M8.2`'s second review
/// round): every other level of this API is linear, and the value itself has
/// to be too. A flush that seals a region, fails to upload, and rebuilds that
/// region with different bytes gets `NonceRegionOutOfOrder` from
/// [`NonceSource::for_region`] — but if the minted nonce were still copyable
/// and in scope, reusing it would compile silently, and two plaintexts under
/// one DEK and one nonce is the exact catastrophe this module exists to make
/// unwritable. A seal takes a `Nonce` **by value** and consumes it.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Nonce([u8; NONCE_BYTES]);

impl Nonce {
    /// The nonce as the twelve bytes AES-GCM takes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; NONCE_BYTES] {
        &self.0
    }
}

/// Twelve bytes read back out of a region header, for **opening** only.
///
/// ⚠️ **A separate type, not a second constructor on [`Nonce`], and that is
/// the point.** `M8.3`/`M8.4` must decode a nonce from a footer to open a
/// sealed region, and the obvious way to write that — `Nonce::from_bytes` —
/// would hand every call site the one constructor this module exists to
/// deny: nothing would then stop a *seal* taking a nonce that came from
/// attacker-influenced bytes, or from a value already used. So decoding
/// lands in its own type with **no path to a `Nonce`** — no `From`, no
/// `into_nonce`, no `TryFrom` — and a sealing API takes `Nonce` while an
/// opening API takes `ParsedNonce`. The compiler keeps the two apart; a
/// comment would not. This type is named here, before `M8.3` needs it, so
/// that task does not reinstate the general constructor for want of a shape.
///
/// ⚠️ **It asserts nothing about what it holds.** It cannot: any twelve bytes
/// are a syntactically valid GCM nonce, and whether they are the *right* ones
/// is answered by the AEAD tag failing, not by this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParsedNonce([u8; NONCE_BYTES]);

impl ParsedNonce {
    /// Reads a nonce out of a region header's bytes.
    #[must_use]
    pub const fn decode(bytes: [u8; NONCE_BYTES]) -> Self {
        Self(bytes)
    }

    /// The bytes, as the AEAD's open takes them.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; NONCE_BYTES] {
        &self.0
    }
}

/// Mints the [`NonceSource`] for every object **one** writer epoch writes.
///
/// ⚠️ **This exists so that no caller can name an object sequence.** An
/// earlier draft let a caller pass one to `NonceSource::new`, and the
/// property test could only avoid the resulting duplicate by keeping the
/// counter itself — which is to say the API had a hole and the test was
/// papering over it. The counter lives here now, advances only, and is the
/// sole way a `NonceSource` comes into being.
///
/// ⚠️ **Not `Clone`, not `Copy`**, for the reason [`NonceSource`] is not: a
/// second minter holding the same counter would re-issue an object sequence
/// its twin had already used.
///
/// ⚠️ One minter per writer epoch, and distinct epochs are the caller's
/// obligation — see the [module docs](self), obligation (1), and `M8.10`.
#[derive(Debug)]
pub struct NonceMinter {
    writer_epoch: u64,
    next_object: u64,
}

impl NonceMinter {
    /// A minter for coordinator-fenced writer incarnation `writer_epoch`.
    ///
    /// # Errors
    ///
    /// [`Error::NonceWriterEpochOutOfRange`] if `writer_epoch` exceeds
    /// [`MAX_WRITER_EPOCH`] — refused, never masked into the field, because a
    /// truncated epoch is exactly how two writers collide.
    pub const fn new(writer_epoch: WriterEpoch) -> Result<Self> {
        if writer_epoch.get() > MAX_WRITER_EPOCH {
            return Err(Error::NonceWriterEpochOutOfRange {
                got: writer_epoch.get(),
            });
        }
        Ok(Self {
            writer_epoch: writer_epoch.get(),
            next_object: 0,
        })
    }

    /// A source for this writer's next object. Each call yields a new one.
    ///
    /// # Errors
    ///
    /// [`Error::NonceObjectSequenceOutOfRange`] once this writer has minted
    /// <code>[MAX_OBJECT_SEQUENCE] + 1</code> objects. ⚠️ An error rather than a wrap,
    /// for the reason [`BundleNamer`](crate::BundleNamer) refuses to wrap its
    /// own sequence: the value after a wrap is one already used, and here that
    /// means every nonce of that object repeats.
    pub const fn next_object(&mut self) -> Result<NonceSource> {
        let object_sequence = self.next_object;
        if object_sequence > MAX_OBJECT_SEQUENCE {
            return Err(Error::NonceObjectSequenceOutOfRange {
                got: object_sequence,
            });
        }
        // Cannot overflow: `object_sequence <= MAX_OBJECT_SEQUENCE`, far below
        // `u64::MAX`, and the next call is refused before it is read again.
        self.next_object = object_sequence + 1;
        Ok(NonceSource::new(self.writer_epoch, object_sequence))
    }

    /// A minter already at `next_object`, for the exhaustion test only.
    ///
    /// ⚠️ `#[cfg(test)]`, so no production caller can reach it. Reaching the
    /// real ceiling takes 2^40 calls, and a ceiling nothing exercises is a
    /// branch nothing constrains.
    #[cfg(test)]
    const fn at(writer_epoch: u64, next_object: u64) -> Self {
        Self {
            writer_epoch,
            next_object,
        }
    }

    /// How many objects this minter has issued a source for.
    #[must_use]
    pub const fn issued(&self) -> u64 {
        self.next_object
    }
}

/// Mints every nonce for the regions of **one** object.
///
/// ⚠️ **Not `Clone`, not `Copy`, and that is the design.** Either derive would
/// give a caller two sources holding the same counter, and the second one's
/// first call would re-mint a nonce the first already handed out. A source is
/// a linear thing: one object, one source, consumed as the regions are sealed.
///
/// See the [module docs](self) for what must hold of `writer_epoch` for this
/// to mean anything.
#[derive(Debug)]
pub struct NonceSource {
    writer_epoch: u64,
    object_sequence: u64,
    /// The only index [`Self::for_region`] will accept next. It only ever
    /// advances, which is what makes a repeat unreachable rather than merely
    /// unlikely.
    next_region: u32,
}

impl NonceSource {
    /// A source for one object of one writer epoch.
    ///
    /// ⚠️ **Private, and infallible, and both are load-bearing.** Private
    /// because a caller able to name an object sequence is a caller able to
    /// name one twice — [`NonceMinter`] is the only way in, and it owns the
    /// counter. Infallible because its two inputs are already in range: the
    /// epoch was checked by [`NonceMinter::new`] and the sequence came from
    /// that minter's own counter, which refuses to pass
    /// [`MAX_OBJECT_SEQUENCE`]. A `Result` here would be a second, unreachable
    /// check that a test could only cover by reaching around the minter.
    const fn new(writer_epoch: u64, object_sequence: u64) -> Self {
        Self {
            writer_epoch,
            object_sequence,
            next_region: 0,
        }
    }

    /// The nonce for the region at position `region_index` in this object.
    ///
    /// # ⚠️ Why a second call with the same index is an *error*
    ///
    /// `ADR-0050` point 2 allows either shape — a counter that cannot go
    /// backwards, or a pure function of the index that returns the same nonce
    /// twice — and this is the counter. The pure-function shape is safe only
    /// while the caller also guarantees it never seals *different* plaintext
    /// under a repeated index, and nothing in the type could check that: a
    /// retried flush that rebuilt its regions slightly differently would reuse
    /// a nonce under one DEK with two plaintexts, which is the catastrophic
    /// case. Refusing the second call moves that from "a caller must not" to
    /// "a caller cannot", and costs nothing, because the index a caller passes
    /// is the region's position and regions are sealed in order.
    ///
    /// # Errors
    ///
    /// [`Error::NonceRegionOutOfRange`] if `region_index` exceeds
    /// [`MAX_REGION_INDEX`], and [`Error::NonceRegionOutOfOrder`] if it is not
    /// the next index this source expects — which is what a repeated call, a
    /// skipped region, or a rewound loop all look like from here.
    pub const fn for_region(&mut self, region_index: u32) -> Result<Nonce> {
        if region_index > MAX_REGION_INDEX {
            return Err(Error::NonceRegionOutOfRange { got: region_index });
        }
        if region_index != self.next_region {
            return Err(Error::NonceRegionOutOfOrder {
                expected: self.next_region,
                got: region_index,
            });
        }
        // Cannot overflow: `next_region <= MAX_REGION_INDEX < u32::MAX`, and
        // the next call is refused as out of range before it is read again.
        self.next_region = region_index + 1;

        let epoch = self.writer_epoch.to_be_bytes();
        let sequence = self.object_sequence.to_be_bytes();
        let region = region_index.to_be_bytes();
        Ok(Nonce([
            // writer epoch, low 40 bits of the big-endian u64
            epoch[3],
            epoch[4],
            epoch[5],
            epoch[6],
            epoch[7],
            // object sequence, low 40 bits of the big-endian u64
            sequence[3],
            sequence[4],
            sequence[5],
            sequence[6],
            sequence[7],
            // region index, low 16 bits of the big-endian u32
            region[2],
            region[3],
        ]))
    }

    /// How many regions this source has minted a nonce for.
    ///
    /// ⚠️ Also the index [`Self::for_region`] will accept next — the two are
    /// the same number by construction, and there is no setter for it.
    #[must_use]
    pub const fn minted(&self) -> u32 {
        self.next_region
    }
}

#[cfg(test)]
mod tests;
