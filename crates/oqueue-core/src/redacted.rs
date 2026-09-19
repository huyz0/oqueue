//! A wrapper that makes its contents unprintable.

use core::fmt;
use subtle::ConstantTimeEq as _;
use zeroize::Zeroize;

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
/// ⚠️ **It still does not zeroize on drop, and `M8.1` did not change that** —
/// it cannot, for two reasons that are structural rather than pending. `Drop`
/// in Rust cannot be implemented conditionally on `T: Zeroize` (that needs
/// specialization), so a `Drop` impl here would have to apply to every `T`,
/// including the many that cannot be wiped; and a type with a `Drop` impl
/// cannot be destructured, which is exactly what [`Redacted::expose_into`]
/// does. What `M8.1` added instead is the narrow half that *is* possible:
/// `impl<T: Zeroize> Zeroize for Redacted<T>`, so an owner holding a
/// `Redacted<Vec<u8>>` can wipe it at a point of its choosing.
///
/// ⚠️ **So `Redacted` is still a formatting guarantee, and key material
/// belongs in [`Dek`](crate::Dek)**, which owns its bytes and wipes them in its
/// own `Drop`. A secret left in a `Redacted<T>` and simply dropped is *not*
/// wiped; `security.md` rule 8 is discharged by the key type, not by this
/// wrapper.
///
/// ⚠️ **Its equality is constant-time, and only exists for byte-like `T`**
/// (`M8.1`, closing the gap the previous text here named). The derived
/// `PartialEq` — which compared bytewise, early-exiting at the first
/// difference and so leaking where two secrets diverge — is replaced by a hand
/// -written one bounded on `T: AsRef<[u8]>`, delegating to [`subtle`]. ⚠️ The
/// *length* is not hidden: `subtle`'s own `ct_eq` on slices answers `false`
/// immediately for unequal lengths, which is inherent to comparing
/// variable-length secrets and is the same behaviour every constant-time
/// library gives.
#[derive(Clone)]
pub struct Redacted<T>(T);

/// ⚠️ Constant-time in the contents. See the type's documentation for what
/// this does and does not hide.
impl<T: AsRef<[u8]>> PartialEq for Redacted<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_ref().ct_eq(other.0.as_ref()).into()
    }
}

impl<T: AsRef<[u8]>> Eq for Redacted<T> {}

/// ⚠️ **Hashes the bytes, on the same bound as [`PartialEq`]** — a derived
/// `Hash` beside a hand-written `PartialEq` is a bug waiting to happen
/// (`clippy::derived_hash_with_manual_eq`), and the two must agree: equal
/// secrets hash equally because both read the same `as_ref()` bytes.
/// ⚠️ Hashing a secret is **not** constant-time and is not meant to be; it is
/// here only so a `Redacted` can sit in a key position, and a caller that
/// hashes key material should think about where the hash goes.
impl<T: AsRef<[u8]>> core::hash::Hash for Redacted<T> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.0.as_ref().hash(state);
    }
}

/// ⚠️ **Wipes on request, never on drop** — see the type's documentation for
/// why a `Drop` impl is not available to this generic wrapper.
impl<T: Zeroize> Zeroize for Redacted<T> {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

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
