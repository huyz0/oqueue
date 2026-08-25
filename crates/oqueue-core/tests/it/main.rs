//! The crate's integration tests, in one binary.
//!
//! `build.md` rule 15: one integration-test binary per crate. Each `tests/*.rs`
//! is otherwise its own crate, its own link, and its own copy of the debuginfo.

mod clock;
mod coordinator;
mod fault;
mod invariants;
mod key;
mod key_layout;
mod metadata_log;
mod multipart;
mod op_counts;
mod precondition;
mod rate_governor;
mod redaction;
mod retry;
mod store;
