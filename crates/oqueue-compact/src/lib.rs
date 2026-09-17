//! Compaction planning and execution: deciding which objects to merge, and
//! doing it.
//!
//! Because objects arrive small and reads want them large, and the gap between
//! those is the whole cost model. Planning is separable from execution and
//! both are testable without touching a network.
#![forbid(unsafe_code)]

mod read_amp;

pub use read_amp::{COMPACTED_OBJECT_RECORDS, ReadAmp, read_amp};
