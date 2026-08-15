//! The offset→object index and its search: given an offset, which object holds
//! it and where inside it.
//!
//! Because a fetch must not enumerate object storage. Doc 12 measures LIST at
//! 12-38x the price of a GET and semantically useless besides, so the index is
//! what turns a read into a bounded number of GETs.
//!
//! ⚠️ **Empty of behaviour.** `M0.8` creates the shape; see this crate's
//! `README.md` for which milestone fills it in.
#![forbid(unsafe_code)]
