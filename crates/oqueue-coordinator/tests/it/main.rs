//! The crate's integration tests, in one binary.
//!
//! `build.md` rule 15: one integration-test binary per crate. Each `tests/*.rs`
//! is otherwise its own crate, its own link, and its own copy of the debuginfo.

mod assignment;
mod cache;
mod chaos;
mod deferred;
mod fencing;
mod follower;
mod idempotence;
mod recovery;
mod sequencing;
mod stamping;
mod standby;
mod support;
mod tail;
mod trim;
mod writer_epoch;
