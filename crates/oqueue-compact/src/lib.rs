//! Compaction planning and execution: deciding which objects to merge, and
//! doing it.
//!
//! Because objects arrive small and reads want them large, and the gap between
//! those is the whole cost model. Planning is separable from execution and
//! both are testable without touching a network.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
