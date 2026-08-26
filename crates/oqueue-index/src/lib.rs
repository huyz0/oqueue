//! The materialization a broker serves reads from, and what fills it.
//!
//! Because a fetch must not enumerate object storage. Doc 12 measures LIST at
//! 12-38x the price of a GET and semantically useless besides, so the index is
//! what turns a read into a bounded number of GETs.
//!
//! ⚠️ **The fold and the search are in `oqueue-core`**, not here — `IndexState`,
//! shared with the [`FakeMaterializedIndex`](oqueue_core::FakeMaterializedIndex)
//! beside the trait, because `contracts.md` rule 9 puts a fake there and two
//! copies of a fold this subtle would drift into two meanings of one contract.
//! `M3.24` records the consequence rather than leaving it to be discovered:
//! [`MemoryIndex`] and that fake are byte-identical today, so the conformance
//! suite proves the contract **once**. `M3.11` is where this crate's type first
//! has what a test double must not — a quota, and an eviction policy.
//!
//! `M3.5` fills in the first of it: [`MemoryIndex`], the materialization a
//! broker serves reads from. ⚠️ It is a **cache** — droppable at any
//! moment, refillable by replaying the log — and the contract it is held to is
//! [`MaterializedIndex`](oqueue_core::MaterializedIndex) in `oqueue-core`.
//!
//! `M3.8` adds what fills it: [`LogApplier`], which folds the metadata log in
//! batches of [`APPLY_BATCH_ENTRIES`] and keeps no bookmark of its own —
//! where to resume is read back out of the index, which advances it inside the
//! same all-or-nothing apply as the entries it covers.
#![forbid(unsafe_code)]

mod applier;
mod memory;

pub use applier::{APPLY_BATCH_ENTRIES, CatchUp, LogApplier};
pub use memory::MemoryIndex;
