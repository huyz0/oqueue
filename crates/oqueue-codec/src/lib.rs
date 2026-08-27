//! The Kafka wire protocol: request and response framing, and `RecordBatch`
//! encode/decode.
//!
//! Because protocol compatibility is the product. Every byte a client sends or
//! expects is decided here, and keeping it in one crate is what makes a
//! golden-byte corpus and a fuzz target possible against a single surface.
//!
//! # `unsafe`
//!
//! Permitted in this crate and budgeted: `check-unsafe.sh` lists it in
//! `ALLOWED_CRATES`, and every block needs an entry in `baselines/unsafe.txt`.
//!
//! ⚠️ **And a differential property test against a safe implementation** —
//! `testing.md` rule 21. Stated separately because it is a separate
//! obligation with a separate enforcer: `check-unsafe.sh` reads the baseline
//! and knows nothing about tests, so folding the two into one sentence made
//! the second look gated when it is review's.
//!
//! ⚠️ **This crate owns the whole codec now (`ADR-0019`).** `ADR-0017` had
//! left message bodies to `kafka-protocol` and this crate hand-rolled only
//! the frame and `RecordBatch` paths; the milestone reversed that after a
//! fuzz-found allocation `DoS`, so [`wire`] and [`flex`] hold the bounded
//! primitives, [`apikey`]/[`frame`] the headers, and the per-message modules
//! ([`apiversions`], [`metadata`], [`produce`], [`fetch`]) the bodies —
//! every one byte-differentialed against `kafka-protocol`, now a test oracle.

pub mod apikey;
pub mod apiversions;
pub mod attributes;
pub mod batch;
pub mod compress;
mod decode_error;
pub mod emit;
pub mod error_codes;
pub mod fetch;
pub mod flex;
pub mod frame;
pub mod listoffsets;
pub mod metadata;
pub mod produce;
pub mod records;
pub mod varint;
pub mod versions;
pub mod wire;

pub use wire::{Cursor, DecodeError};
