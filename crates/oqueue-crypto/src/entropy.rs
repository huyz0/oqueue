//! Where a fresh data encryption key's 32 bytes come from.
//!
//! # ⚠️ Why this is a seam and not a call to the OS
//!
//! Non-negotiable 5 says business logic is sans-I/O, and a read of the
//! operating system's random pool is I/O: it is a syscall whose result no test
//! can predict and no test can reproduce. A [`DekCache`](crate::DekCache) that
//! called `getrandom` directly would be untestable in exactly the way the
//! clock seam exists to prevent — the cache's rotation logic is the thing
//! `NFR-33` is about, and asserting it must not depend on what the kernel
//! happened to return.
//!
//! So the same shape as [`Clock`](oqueue_core::Clock): a trait, a real
//! implementation, and a fake that lives beside it (`contracts.md` rule 9) so
//! a downstream test needs no testkit dependency.
//!
//! ⚠️ **The seam is deliberately narrow — exactly one method, returning
//! exactly [`DEK_BYTES`] bytes.** A general "give me `n` random bytes" API
//! would invite a second caller, and this project's *nonces* are constructed
//! rather than random (`oqueue_core::nonce`, `ADR-0050` point 2). A random
//! nonce is the one catastrophic mistake `M8` is built to make unwritable, and
//! a general randomness API in scope is how it would get written.
//!
//! ⚠️ **Never derive a DEK from a counter.** [`FakeEntropy`] does exactly that
//! and is a test double for that reason; its own documentation says so at
//! length.

use oqueue_core::{DEK_BYTES, Dek, Error, Result};
use zeroize::Zeroize as _;

/// Produces the key material a fresh data encryption key is made of.
///
/// # What an implementor must guarantee
///
/// - **Unpredictable.** An observer of every previous output learns nothing
///   about the next one. This is the whole contract; a counter, a hash of the
///   time, or a seeded PRNG whose seed is derivable all fail it, and a DEK
///   built from any of them is a DEK an attacker can reconstruct without the
///   KEK.
/// - **Independent per call.** Two calls never agree except by the chance two
///   uniform 32-byte draws agree.
/// - **Cheap enough to call on a rotation**, which is once per topic per
///   64 GiB or 7 days — not once per region, and never on the produce path.
///
/// ⚠️ **An implementation that cannot meet the first guarantee must return an
/// error, never weaker bytes.** A DEK is the only thing between a tenant's
/// records and anyone holding the object; degrading to a predictable source is
/// a silent, durable compromise, which is the same failure
/// [`NoOpKeyProvider`](crate::NoOpKeyProvider) refuses rather than fakes.
pub trait Entropy: Send + Sync + core::fmt::Debug {
    /// Fills `out` with [`DEK_BYTES`] unpredictable bytes.
    ///
    /// # Errors
    ///
    /// [`Error::EntropyUnavailable`] if no unpredictable bytes can be
    /// produced. ⚠️ The error carries nothing about the source's state, for
    /// the reason every error in this area carries nothing: it is rendered
    /// into an operator's log.
    fn fill(&self, out: &mut [u8; DEK_BYTES]) -> Result<()>;
}

/// Mints a fresh [`Dek`] from `entropy`.
///
/// ⚠️ **The only constructor of a live DEK in this crate.** The scratch buffer
/// is zeroized before it is dropped, so the only surviving copy is the one the
/// returned `Dek` owns and zeroizes itself (`security.md` rule 8).
///
/// # Errors
///
/// Whatever [`Entropy::fill`] returned.
pub fn mint_dek(entropy: &dyn Entropy) -> Result<Dek> {
    let mut bytes = [0u8; DEK_BYTES];
    let filled = entropy.fill(&mut bytes);
    // ⚠️ Before the `?`, so a failing source leaves nothing behind either.
    let dek = filled.map(|()| Dek::new(bytes));
    bytes.zeroize();
    dek
}

/// The operating system's random source.
///
/// ⚠️ **Constructed at the composition root only** (`bin/oqueue`, or a
/// broker assembling its write path), and named here because this is where the
/// trait is. Nothing in a library's own logic should reach for it: a function
/// that takes `&dyn Entropy` is testable and a function that builds an
/// `OsEntropy` is not.
///
/// Backed by `getrandom`, which is the platform's own CSPRNG —
/// `getrandom(2)` on Linux, `arc4random_buf` on the BSDs and macOS,
/// `ProcessPrng` on Windows. ⚠️ **Not a userspace PRNG seeded once**: there is
/// no seed here to leak, fork, or replay across a process image that was
/// snapshotted and resumed, which a long-lived broker in a VM is a real
/// candidate for.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsEntropy;

impl OsEntropy {
    /// The operating system's random source.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Entropy for OsEntropy {
    fn fill(&self, out: &mut [u8; DEK_BYTES]) -> Result<()> {
        // ⚠️ The error is discarded rather than wrapped: `getrandom::Error` is
        // an opaque OS code, and `error-handling.md` rule 5 classifies rather
        // than wraps. There is exactly one thing a caller does about it.
        getrandom::fill(out).map_err(|_| Error::EntropyUnavailable)
    }
}

/// An [`Entropy`] whose output is a counter, for tests.
///
/// ⚠️ **This produces predictable key material and is not entropy at all.** It
/// exists so a test can assert *which* DEK the cache is holding — which is how
/// `M8.5`'s rotation tests observe that the old key was dropped — and for no
/// other reason. Constructing one outside a test writes guessable DEKs into
/// object storage, which is the compromise the whole envelope exists to
/// prevent.
///
/// It lives here rather than in `oqueue-testkit` for the reason
/// [`FakeClock`](oqueue_core::FakeClock) lives beside `Clock`: `contracts.md`
/// rule 9, so a crate can test its own time- and key-dependent logic without
/// taking a test dependency.
///
/// Each call returns a distinct block: the call ordinal in the first eight
/// bytes, a fixed filler in the rest.
#[derive(Debug, Default)]
pub struct FakeEntropy {
    calls: core::sync::atomic::AtomicU64,
}

impl FakeEntropy {
    /// A fake whose first block is ordinal `0`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            calls: core::sync::atomic::AtomicU64::new(0),
        }
    }

    /// How many blocks it has handed out.
    ///
    /// ⚠️ A test asserting on this is asserting on *mints*, which is not the
    /// same question as "how many times was the KMS called" — that one is
    /// answered by counting [`KeyProvider::wrap`](oqueue_core::KeyProvider)
    /// instead, and `NFR-33` is about the second.
    #[must_use]
    pub fn calls(&self) -> u64 {
        self.calls.load(core::sync::atomic::Ordering::SeqCst)
    }

    /// The block this fake returns for call ordinal `n`, so a test can name a
    /// DEK it expects without having taken it out of the cache.
    ///
    /// ⚠️ **Written as one slice copy rather than a `while` loop**, and that is
    /// not style: the loop's `i += 1` is a line a mutation testing tool turns
    /// into `i *= 1`, which never terminates — a hang in the test suite rather
    /// than a failure, and a survivor nothing can argue away. A form with no
    /// loop counter has no such mutant.
    #[must_use]
    pub fn block(n: u64) -> [u8; DEK_BYTES] {
        let mut out = [0xA5u8; DEK_BYTES];
        out[..8].copy_from_slice(&n.to_be_bytes());
        out
    }
}

impl Entropy for FakeEntropy {
    fn fill(&self, out: &mut [u8; DEK_BYTES]) -> Result<()> {
        let n = self
            .calls
            .fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        *out = Self::block(n);
        Ok(())
    }
}
