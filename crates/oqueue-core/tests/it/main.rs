//! The crate's integration tests, in one binary.
//!
//! `build.md` rule 15: one integration-test binary per crate. Each `tests/*.rs`
//! is otherwise its own crate, its own link, and its own copy of the debuginfo.

mod batched_effects;
mod bundle;
mod bundle_footer;
mod clock;
mod composite;
mod coordinator;
mod coverage;
mod fault;
mod footer_tail;
mod index_cost;
mod index_growth;
mod index_reader;
mod invariants;
mod key;
mod key_layout;
mod manifest_paging;
mod manifest_published;
mod materialized_index;
mod metadata_log;
mod multipart;
mod object_ref;
mod op_counts;
mod partition_manifest;
mod precondition;
mod range_compacted;
mod rate_governor;
mod redaction;
mod retry;
mod store;
mod store_streaming;
mod tiers;
