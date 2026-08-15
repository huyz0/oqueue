//! The I/O shell: the connection loop, request dispatch, and the batching that
//! turns many small produces into few large PUTs.
//!
//! Because everything else in this workspace is sans-I/O by construction, and
//! the sockets have to be somewhere. This crate is that somewhere, and it is
//! generic over its seams so it can be tested without any of them being real.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
