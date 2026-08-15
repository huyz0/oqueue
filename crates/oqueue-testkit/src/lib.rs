//! Test harness and data generators. No fakes.
//!
//! Because the property tests and the conformance suites need seeded
//! generators and a common harness, and duplicating those across ten crates
//! would guarantee they drift.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
