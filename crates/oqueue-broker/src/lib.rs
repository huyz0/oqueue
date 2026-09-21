//! The I/O shell: the connection loop, request dispatch, and the batching that
//! turns many small produces into few large PUTs.
//!
//! ⚠️ Because *almost* everything else in this workspace is sans-I/O — two
//! exceptions, not one (`M1.38`): `oqueue-store`, exempt from
//! `check-sans-io.sh`'s object-storage pattern because it implements those
//! backends, and `bin/oqueue`, which the gate never scans at all since it
//! walks `crates/*.rs` only. Every *other* library crate is held to all three
//! patterns; this crate is exempt from all three, which is the point of it —
//! and
//! the sockets have to be somewhere. This crate is that somewhere, and it is
//! generic over its seams so it can be tested without any of them being real.
//!
//! `M2.17` began filling it: [`connection`] is the per-connection task —
//! framed reads, pipelined handlers, in-order writes — generic over the
//! stream so every test drives a `tokio::io::duplex` and no real socket
//! exists below `bin/oqueue`.
//!
//! ⚠️ **`M3.14` replaced the stub it served against.** [`Cluster`] composes a
//! real `Coordinator`, a read-only index and an `ObjectStore`: a produce seals
//! one bundle, PUTs it once and commits its spans, and a fetch resolves
//! offset→object through the index. What is still a stand-in is *below* the
//! seams — M3 builds no durable metadata log (`M6`), so a restart re-bases at
//! `Offset::ZERO` over objects that already hold those offsets. That is
//! recorded in `M3.md`'s goal and `roadmap.md`'s deferral table rather than
//! left for an operator to discover.
#![forbid(unsafe_code)]

mod alter_configs;
mod authz;
pub mod checkpoint;
pub mod cluster;
pub mod connection;
mod create_topics;
mod delete_topics;
mod describe_configs;
pub mod dispatch;
mod fencing;
pub mod fetch;
mod find_coordinator;
mod flush;
mod group_admin;
mod group_transitions;
mod heartbeat;
mod incremental_alter_configs;
mod ingest;
mod init_producer_id;
mod join_group;
pub mod lease_renewal;
mod leave_group;
pub mod listoffsets;
pub mod metadata;
mod offset_commit;
mod offset_fetch;
pub mod produce;
mod quota_admin;
mod read;
mod region;
mod region_sealer;
mod replay_gate;
pub mod retention;
pub mod sasl_authenticate;
mod sasl_handshake;
pub mod session;
mod sync_group;
mod telemetry;
#[cfg(test)]
mod testing;
pub mod tls;
mod writer_id;

/// How long a degraded node waits between attempts to reach what it could
/// not reach at boot — its metadata log, its group log (`M6.10`).
///
/// ⚠️ **UNDERIVED**: short enough that recovery follows the store within a
/// second, long enough that an unreachable store costs one request a second.
pub const DEGRADED_RETRY: core::time::Duration = core::time::Duration::from_secs(1);

pub use checkpoint::{CHECKPOINT_JOURNAL_BYTES, CHECKPOINT_PAUSE, CurrentLog, checkpoints};
pub use cluster::{Cluster, FlushError, Seams, Sequencing};
pub use connection::{ConnectionEnd, ConnectionLimits, Handler, HandlerResponse, serve_connection};
pub use dispatch::Dispatcher;
pub use fetch::{Allowance, MAX_PARK_MS};
pub use lease_renewal::{keep_lease, renew_lease};
pub use read::MAX_FAILED_FETCHES_PER_REQUEST;
pub use region_sealer::{RegionSealer, RejectingRegionSealer, SealedRegionOwned};
pub use retention::{RETENTION_ROUND_INTERVAL, Retention};
pub use sasl_authenticate::{PlainCredential, PlainCredentials};
pub use session::Session;
pub use writer_id::WriterId;
