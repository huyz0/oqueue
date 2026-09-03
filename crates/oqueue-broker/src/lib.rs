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

mod authz;
pub mod cluster;
pub mod connection;
pub mod dispatch;
mod fencing;
pub mod fetch;
mod find_coordinator;
mod flush;
mod heartbeat;
mod ingest;
mod init_producer_id;
mod join_group;
mod leave_group;
pub mod listoffsets;
pub mod metadata;
mod offset_commit;
pub mod produce;
mod read;
mod region;
pub mod sasl_authenticate;
mod sasl_handshake;
pub mod session;
mod sync_group;
#[cfg(test)]
mod testing;
pub mod tls;
mod writer_id;

pub use cluster::{Cluster, FlushError, Seams, Sequencing};
pub use connection::{ConnectionEnd, ConnectionLimits, Handler, HandlerResponse, serve_connection};
pub use dispatch::Dispatcher;
pub use fetch::{Allowance, MAX_PARK_MS};
pub use read::MAX_FAILED_FETCHES_PER_REQUEST;
pub use sasl_authenticate::{PlainCredential, PlainCredentials};
pub use session::Session;
pub use writer_id::WriterId;
