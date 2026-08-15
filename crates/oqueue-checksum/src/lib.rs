//! CRC-32C (Castagnoli), with runtime dispatch to whatever the CPU supports.
//!
//! Because Kafka's v2 `RecordBatch` mandates CRC-32C and the spread between a
//! scalar and a hardware implementation is large enough to matter on every
//! batch. One crate, one algorithm, one known-answer test.
//!
//! # `unsafe`
//!
//! Permitted in this crate and budgeted: `check-unsafe.sh` lists it in
//! `ALLOWED_CRATES`, and every block needs an entry in `baselines/unsafe.txt`
//! plus a differential property test against a safe implementation.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
