//! Metadata, offset sequencing, and recovery.
//!
//! Because offsets must be totally ordered per partition and object storage
//! offers no atomic append. Sequencing is the one place that ordering is
//! decided, and M3 is where it is built.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
