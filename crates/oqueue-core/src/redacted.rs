//! A wrapper that makes its contents unprintable.

use core::fmt;

/// Holds a value that must never reach a log, span, metric label, or error
/// message (FR-44).
///
/// # Invariant
///
/// **Neither `Debug` nor `Display` reproduces any part of the wrapped value**,
/// and there is no formatting impl that does. The only way to the value is
/// [`Redacted::expose`], which a reader can grep for.
///
/// ⚠️ **The mechanism is the missing bound, not the impls.** Neither
/// `impl` below requires `T: Debug` or `T: Display`, so the wrapped type's own
/// formatting is not merely unused — it is *unreachable from here*, and a
/// future edit that tried to print `self.0` would not compile without adding a
/// bound that does not exist. `security.md` rule 7 says nothing holding a
/// secret derives `Debug`; this is that rule expressed as a type, so a struct
/// with a `Redacted` field may derive `Debug` safely.
///
/// # What this does not do
///
/// ⚠️ **It does not zeroize on drop.** `security.md` rule 8 requires that of key
/// material, and it needs a `T` bound this generic wrapper does not impose. Key
/// types arrive with `M0.11`'s `KeyProvider` and `M8`'s crypto work, and that is
/// where the obligation lands. A `Redacted<T>` is a *formatting* guarantee and
/// nothing more.
///
/// ⚠️ **Its `PartialEq` is not constant-time.** Comparing two wrapped secrets
/// leaks timing. Nothing in `M0` compares secrets; `M8` needs a constant-time
/// equality before anything does.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    /// Wraps a value.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Borrows the wrapped value.
    ///
    /// ⚠️ Named to be conspicuous in a diff and in a grep. Every call is a place
    /// where a secret leaves its wrapper, and that is a review question.
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Takes the wrapped value.
    ///
    /// ⚠️ Same caution as [`Redacted::expose`].
    pub fn expose_into(self) -> T {
        self.0
    }
}

/// ⚠️ Prints a fixed string. No `T: Debug` bound, deliberately — see the type's
/// documentation.
impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Redacted(<redacted>)")
    }
}

/// ⚠️ Prints a fixed string. No `T: Display` bound, deliberately.
///
/// Uses [`fmt::Formatter::pad`] so width, fill and alignment behave as a reader
/// of `{:>40}` expects. ⚠️ That is safe precisely because what it pads is a
/// constant: `pad` can truncate under a precision specifier, and truncating
/// `"<redacted>"` yields a shorter fixed string, never a fragment of `T`.
impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad("<redacted>")
    }
}
