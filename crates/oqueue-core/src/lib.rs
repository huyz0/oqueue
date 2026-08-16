//! Types, IDs, errors, and every trait seam the rest of the workspace is
//! written against.
//!
//! This is the one crate everything depends on, and the one crate that depends
//! on no other crate here — the centre of `architecture.md`'s star. It performs
//! no I/O, reads no clock, and names no socket or object store; anything that
//! needs those is expressed as a trait seam whose implementations live
//! downstream (NFR-51).
//!
//! # What is here so far
//!
//! The four core identifiers ([`TopicId`], [`PartitionId`], [`Offset`],
//! [`ObjectKey`]), the [`Error`] enum they can produce, and [`Redacted`] — the
//! wrapper that makes a secret unprintable so FR-44 holds by construction
//! rather than by everyone remembering. All three trait seams — [`Clock`],
//! [`ObjectStore`] and [`KeyProvider`] — are here, each with its fake beside
//! it. ⚠️ Shaped as ADR-0002 decided, which is a rule about the **async**
//! seams: [`ObjectStore`] and [`KeyProvider`] are one `dyn`-compatible trait
//! each, returning a hand-written boxed future. [`Clock`] is **not async and
//! needs no boxing at all** — ADR-0002 says so in as many words, and ADR-0004
//! rejects a per-call allocation on it by name.
//!
//! # The property every identifier here shares
//!
//! ⚠️ **Each one's invariant is unconstructible-around**, not merely checked at
//! the door. The field is private, the only constructor validates, and there is
//! no setter — so a value that violates the invariant is not a value this crate
//! can hand out. That is what lets everything downstream stop re-checking.
#![forbid(unsafe_code)]

mod clock;
mod error;
mod key;
mod object_key;
mod offset;
mod partition;
mod redacted;
mod store;
mod topic;

pub use clock::{Clock, FakeClock, Timestamp};
pub use error::{Error, Result};
pub use key::{FakeKeyProvider, KeyId, KeyProvider, WrappedKey};
pub use object_key::ObjectKey;
pub use offset::Offset;
pub use partition::PartitionId;
pub use redacted::Redacted;
pub use store::{BoxFuture, FakeObjectStore, ObjectStore};
pub use topic::TopicId;
