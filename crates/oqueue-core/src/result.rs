//! The crate's result alias.
//!
//! ⚠️ **Its own file so that `error.rs` holds exactly one item** (`M5.42`,
//! `ADR-0040`). That enum is exempt from `code-structure.md` rule 16's line
//! limit as a *list* — one flat sequence of independent variants, which is
//! rule 17's generated-table case — and `check-file-size.sh` keeps that
//! exemption honest structurally rather than numerically: a file holding a
//! second top-level item is a module again, and the limit applies to it. A
//! two-line type alias is not what made the enum long, and leaving it there
//! would have made the condition unsatisfiable for a reason unrelated to why
//! the exemption exists.

use crate::Error;

/// The crate's result alias.
pub type Result<T> = core::result::Result<T, Error>;
