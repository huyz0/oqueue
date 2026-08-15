//! Buffer primitives: refcounted slices, and the pooling that keeps record
//! bytes from being copied on every hop.
//!
//! Because a broker's throughput is decided by how many times a byte is copied
//! between the socket and the object store, and the answer has to be *once*.
//! Centralising the buffer type is what lets every other crate hand bytes
//! along without owning them.
//!
//! # `unsafe`
//!
//! Permitted in this crate and budgeted: `check-unsafe.sh` lists it in
//! `ALLOWED_CRATES`, and every block needs an entry in `baselines/unsafe.txt`
//! plus a differential property test against a safe implementation.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
