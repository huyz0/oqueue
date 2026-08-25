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
#![forbid(unsafe_code)]

mod memory;

pub use memory::MemoryIndex;
