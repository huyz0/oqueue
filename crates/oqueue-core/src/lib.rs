//! Types, IDs, errors, and every trait seam in oqueue.
//!
//! This crate is the centre of the workspace's star topology: every other crate
//! depends on it, and it depends on nothing. That is what keeps the build DAG
//! two links deep however many crates are added sideways, and what lets each of
//! them be tested without constructing the system.
//!
//! Nothing here performs I/O, reads the real clock, or names an async runtime.
//! Those arrive through the seams defined here and are supplied by
//! `oqueue-broker` and `bin/oqueue`.
//!
//! It is empty for now. `M0.5` brings the core types and IDs, `M0.6` the error
//! taxonomy, and `M0.9` through `M0.11` the three seams, each with its fake
//! beside it — `contracts.md` rule 9 puts fakes here rather than in
//! `oqueue-testkit`, so that a crate can be tested without a testkit dependency.

#![forbid(unsafe_code)]
