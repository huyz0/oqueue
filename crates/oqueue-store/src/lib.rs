//! `ObjectStore` implementations: S3, GCS, and an in-memory backend.
//!
//! Because the seam is in `oqueue-core` and the concrete backends must live
//! somewhere that is not `oqueue-core`. This crate is where a vendor SDK is
//! allowed to appear.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
