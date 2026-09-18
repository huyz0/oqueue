//! `oqueue-broker`'s integration tests — T1: a runtime and a duplex pipe,
//! no socket, no container.
//!
//! ⚠️ The simulated socket of `ADR-0027`'s three-piece harness. The other
//! two are `oqueue-testkit` (seed, schedule, invariants) and `oqueue-store`'s
//! simulated S3 (`crates/oqueue-store/tests/it/sim.rs`).

mod budget;
mod connection;
mod corpus;
mod crash_points;
mod cross_principal;
mod faults;
mod generated;
mod invariants;
mod manifest_counts;
mod manifest_reads;
mod matrix;
mod metadata_cost;
mod overlap;
mod quota;
mod reads;
mod reap;
mod roundtrip;
mod secrets_log_scan;
mod seeded;
mod support;
mod virtual_time;
