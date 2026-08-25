//! The crate's integration tests, in one binary.
//!
//! `build.md` rule 15: one integration-test binary per crate. Each `tests/*.rs`
//! is otherwise its own crate, its own link, and its own copy of the debuginfo.

mod applier;
mod index;
