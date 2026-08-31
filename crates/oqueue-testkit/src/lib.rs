//! Test harness and data generators. No fakes.
//!
//! Because the property tests and the conformance suites need seeded
//! generators and a common harness, and duplicating those across ten crates
//! would guarantee they drift.
//!
//! ⚠️ **`M10.4` is the milestone that filled it.** `M0.8` created the shape and
//! this crate held nothing until the simulation harness needed somewhere every
//! crate's tests could reach — `ADR-0027` places the seed, the schedule and
//! the invariant checks here, and the other two pieces in `oqueue-store`'s and
//! `oqueue-broker`'s own `tests/` trees.
#![forbid(unsafe_code)]

pub mod seed;

pub use seed::{run_seeded, seeded_runtime};
