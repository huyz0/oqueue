//! `ObjectStore` implementations: S3, GCS, and an in-memory backend.
//!
//! Because the seam is in `oqueue-core` and the concrete backends must live
//! somewhere that is not `oqueue-core`. This crate is where a vendor SDK is
//! allowed to appear.
#![forbid(unsafe_code)]

mod classify;
mod gcs;
mod get;
mod multipart;
mod s3;
mod tls;

pub use gcs::GcsStore;
pub use s3::S3Store;
