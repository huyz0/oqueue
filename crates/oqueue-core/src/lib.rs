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
//! [`ObjectKey`]) and the [`Error`] enum they can produce. The trait seams
//! (`Clock`, `ObjectStore`, `KeyProvider`) arrive in `M0.9` through `M0.11`,
//! shaped as ADR-0002 decided.
//!
//! # The property every identifier here shares
//!
//! ⚠️ **Each one's invariant is unconstructible-around**, not merely checked at
//! the door. The field is private, the only constructor validates, and there is
//! no setter — so a value that violates the invariant is not a value this crate
//! can hand out. That is what lets everything downstream stop re-checking.
#![forbid(unsafe_code)]

mod error;
mod object_key;
mod offset;
mod partition;
mod topic;

pub use error::{Error, Result};
pub use object_key::ObjectKey;
pub use offset::Offset;
pub use partition::PartitionId;
pub use topic::TopicId;
