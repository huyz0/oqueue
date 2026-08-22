//! `ObjectStore` implementations: S3 and GCS.
//!
//! ⚠️ **Not an in-memory one**, which this line claimed until `M1.37`: the
//! in-memory implementation is `oqueue-core`'s `FakeObjectStore`, which since
//! `M1.8` carries a `FaultConfig` and since `M1.10` passes this crate's own
//! conformance suite at `Capabilities::FULL`. `ADR-0005` had planned a second,
//! separate one here; there was nothing left for it to do.
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
