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
#![forbid(unsafe_code)]

pub mod connection;
pub mod dispatch;
pub mod metadata;
pub mod stub;

pub use connection::{ConnectionEnd, ConnectionLimits, Handler, serve_connection};
pub use dispatch::Dispatcher;
pub use stub::StubCluster;
