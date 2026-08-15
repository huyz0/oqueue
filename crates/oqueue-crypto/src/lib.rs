//! AEAD, envelope encryption, the DEK cache, and nonce construction.
//!
//! Because BYOK is a per-topic opt-in and server-side encryption cannot
//! express it: one object carries many tenants' data and can only carry one
//! SSE-KMS key. So encryption is broker-side, and this is where it lives.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
