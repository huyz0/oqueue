//! The offset→object index and its search: given an offset, which object holds
//! it and where inside it.
//!
//! Because a fetch must not enumerate object storage. Doc 12 measures LIST at
//! 12-38x the price of a GET and semantically useless besides, so the index is
//! what turns a read into a bounded number of GETs.
//!
//! `M3.5` fills in the first of it: [`MemoryIndex`], the in-memory fold of the
//! metadata log that a fetch queries. ⚠️ It is a **cache** — droppable at any
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
