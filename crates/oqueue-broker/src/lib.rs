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
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
