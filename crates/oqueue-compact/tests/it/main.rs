//! The crate's integration tests, in one binary.
//!
//! `build.md` rule 15: one integration-test binary per crate. Each `tests/*.rs`
//! is otherwise its own crate, its own link, and its own copy of the debuginfo.

mod compose;
mod encryption;
mod end_to_end;
mod gc_race;
mod indexes;
mod layout;
mod lifecycle;
mod merge;
mod merge_refusals;
mod naming;
mod read_amp;
mod retention;
mod support;
mod sweep;
mod trimmed;
