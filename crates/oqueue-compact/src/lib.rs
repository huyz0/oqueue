//! Compaction planning and execution: deciding which objects to merge, and
//! doing it.
//!
//! Because objects arrive small and reads want them large, and the gap between
//! those is the whole cost model. Planning is separable from execution and
//! both are testable without touching a network.
#![forbid(unsafe_code)]

mod compose;
mod cost;
mod gc;
mod layout;
mod lifecycle;
mod merge;
mod namer;
mod plan;
mod read_amp;
mod retention;
mod sweep;

pub use compose::{ComposeOutcome, compose, read_composite};
pub use cost::{COMPACTION_PLAN_RECORDS_BUDGET, CostEstimate};

pub use gc::{DELETION_DELAY_MS, GcTerms, MAX_CLOCK_SKEW_MS, MAX_IN_FLIGHT_FETCH_MS};
pub use layout::{PlannedInputs, merge_round};
pub use lifecycle::{
    DELETE_BATCH_KEYS, DELETION_BACKLOG_KEYS, Lifecycle, QUARANTINE_AFTER_REFUSALS, SweepReport,
};
pub use merge::{MergeOutcome, merge};
pub use namer::CompactionNamer;
pub use plan::{COMPACTION_READ_AMP_THRESHOLD, CompactionPlan, Planning, plan};
pub use read_amp::{COMPACTED_OBJECT_RECORDS, ReadAmp, read_amp};
pub use retention::{DEFAULT_RETENTION_MS, ExpiryHeap};
pub use sweep::{COMPACTION_SWEEP_INTERVAL, Candidate, Sweep, sweep};
