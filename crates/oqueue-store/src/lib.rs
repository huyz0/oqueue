//! `ObjectStore` implementations: S3 and GCS.
//!
//! ⚠️ **Not an in-memory one**, which this line claimed until `M1.37`: the
//! in-memory implementation is `oqueue-core`'s `FakeObjectStore`, which since
//! `M1.8` carries a `FaultConfig` and since `M1.10` passes this crate's own
//! conformance suite at `Capabilities::FULL`. `ADR-0005` had planned a second,
//! separate one here; there was nothing left for it to do.
//!
//! ⚠️ **`FULL` means one more thing since `M3.15`**: `injectable_ack_loss` —
//! being able to be *told* to lose a `put`'s acknowledgement after the bytes
//! are durable, which is `ADR-0005` **guarantee 2**'s unknown state. `S3Store`
//! declares it off, so the case that needs it is skipped and *named in the
//! report* rather than passed without running — the explicit skip `M1.37`
//! found was happening by omission.
//!
//! ⚠️ **It is a harness capability and not a durability one.** S3 is the
//! backend that actually provides guarantee 1; what it lacks is a way to be
//! told to misbehave. And guarantee 1's *crash* clause — `Ok` surviving a
//! process death — no single-process fake can show at all, so it stays
//! `M15`'s (`M1.44`).
//!
//! Because the seam is in `oqueue-core` and the concrete backends must live
//! somewhere that is not `oqueue-core`. This crate is where a vendor SDK is
//! allowed to appear.
#![forbid(unsafe_code)]

mod classify;
mod gcs;
mod get;
mod list;
mod multipart;
mod retry;
mod s3;
mod s3_stream;
mod tls;

pub use gcs::GcsStore;
pub use retry::retry_config_for;
pub use s3::S3Store;
