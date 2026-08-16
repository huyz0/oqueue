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
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
