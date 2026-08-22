//! The oqueue broker.
//!
//! ⚠️ **This is the composition root, and the only place in the workspace where
//! a concrete implementation is chosen** (FR-50, NFR-51). The decision about
//! which `ObjectStore`, which `Clock` and which `KeyProvider` the seams in
//! `oqueue-core` resolve to is made here and nowhere else.
//!
//! ⚠️ ~~Every library crate is written against the trait seams in
//! `oqueue-core`.~~ — **a rule with named exceptions, not a property of every
//! crate** (`M1.38`, mirroring the same correction `M1.28` made to this
//! crate's `README.md`). `scripts/check-sans-io.sh` draws the actual line:
//! `oqueue-store` is exempt from the object-storage pattern because it is the
//! crate that implements those backends, and `oqueue-broker` from all three
//! because the I/O shell is meant to live there. Every other library crate is
//! held to all three. ⚠️ `bin/` is not scanned at all, so nothing in this file
//! is held to any of them — the Invariants table in `README.md` says so too.
//!
//! ⚠️ **This did not spread from anywhere**, which a first version of `M1.38`
//! claimed and git refutes: the sentence is in this file from `M0.12`
//! (`b84092b`), *before* `M0.31`, and `git log -S` finds it never in
//! `oqueue-broker`'s README at all. **Five** sentences making the same universal
//! claim exist, and they were written in **two** commits rather than
//! independently: `M0.8` (`fabd3fe`) stamped the broker's `lib.rs` and
//! `README.md`; `M0.12` (`b84092b`) stamped this file, `bin/oqueue/AGENTS.md`
//! and `bin/oqueue/README.md`. Four are removed in this one commit, the fifth
//! by `M1.28`. ⚠️ **A crate-skeleton commit stamps its universal into every
//! file it creates**, so the copies are as numerous as the files and the two
//! batches share no phrasing — grep from either finds nothing of the other.
//! This wants a gate, not a sixth sweep.
//!
//! ⚠️ **It starts, prints a version, and exits.** There is no broker yet:
//! `M2` brings the wire protocol and `oqueue-broker` the I/O shell that runs
//! it. What exists here is the shape — the place the wiring will go, and the
//! proof that `check-layering.sh` treats this package as a composer.
#![forbid(unsafe_code)]

// ⚠️ **The global allocator, set here and in no library crate** — ADR-0007.
// A library that sets one imposes it on every consumer, including tests and
// benchmarks that did not ask for it.
#[cfg(not(feature = "heap-profiling"))]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// ⚠️ Behind a non-default feature because jemalloc bakes the page size in at
/// build time: a binary built assuming 4 KB **aborts at startup** on a 64 KB-page
/// aarch64 kernel, and NFR-40 makes aarch64 first-class. Build with
/// `JEMALLOC_SYS_WITH_LG_PAGE=16` for those hosts. doc 18 §3.5.
#[cfg(feature = "heap-profiling")]
#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use oqueue_core::KeyProvider;
use oqueue_crypto::NoOpKeyProvider;

/// The concrete choices this deployment runs with.
///
/// ⚠️ This is the composition root's entire purpose, and today it has one
/// member.
///
/// ⚠️ ~~`Clock` and `ObjectStore` join it when a real implementation of either
/// exists — `M1` writes the first.~~ — **`M1.15` and `M1.17` wrote `S3Store`
/// and `GcsStore`, so the stated condition is met and this field did not
/// appear** (`M1.39`). The condition was the wrong one: an implementation
/// existing is not a reason to wire it. A `Wiring` member with no caller is
/// unearned infrastructure, and `bin/oqueue` has no `oqueue-store` dependency
/// precisely so that adding one is a deliberate act.
///
/// `ObjectStore` joins when this binary must *hand* one to something —
/// `M3`, the first milestone where a record is acknowledged only after it is
/// in object storage. ⚠️ Not "when this binary performs I/O" — it does write
/// the banner to stdout — but it performs no *object-storage or network* I/O:
/// a composition root constructs and injects, and the object-storage call
/// happens behind the seam. `Clock` joins on the same rule, when something it
/// wires schedules or expires.
///
/// ⚠️ **Not because "nothing would observe it."** That argument does not hold
/// here: the banner prints the chosen `KeyProvider`, and
/// `tests/it/startup.rs` pins that line byte-for-byte, so a wired member can
/// be observed. The reason is the one
/// stated above — no *caller* needs the value, so wiring it would be unearned
/// infrastructure.
struct Wiring {
    keys: Box<dyn KeyProvider>,
}

impl Wiring {
    /// The key provider this deployment runs with.
    fn keys(&self) -> &dyn KeyProvider {
        self.keys.as_ref()
    }

    /// The default deployment: no BYOK.
    ///
    /// ⚠️ `NoOpKeyProvider` **refuses** wrap and unwrap rather than passing
    /// plaintext through — ADR-0006. That is the correct default: a deployment
    /// that has not configured a KMS should fail the first encryption call, not
    /// silently write a plaintext data encryption key to object storage.
    fn unencrypted() -> Self {
        Self {
            keys: Box::new(NoOpKeyProvider::new()),
        }
    }
}

fn main() {
    // The composition root's job, in the order it will always happen: choose
    // the concrete implementations, then hand them to the shell that runs them.
    let wiring = Wiring::unencrypted();
    run(&wiring);
}

/// Runs the broker.
///
/// ⚠️ There is no broker. `M2` brings the wire protocol and `oqueue-broker` the
/// I/O shell; this is the seam in the *process* where that shell will be
/// started, and taking `wiring` by reference now means the signature does not
/// change when it is.
fn run(wiring: &Wiring) {
    // ⚠️ `println!` rather than `tracing`. `rust-style.md` rule 12 allows the
    // macros in `bin/oqueue` only before `tracing` is initialised, and nothing
    // initialises it yet — a version line is exactly the "before" case.
    // ⚠️ The banner names the chosen implementation, which is the one thing a
    // composition root has to say. It is also what keeps the choice honest:
    // swapping in a different `KeyProvider` changes observable output, so the
    // wiring cannot rot into something nothing reads.
    println!("oqueue {} ({:?})", env!("CARGO_PKG_VERSION"), wiring.keys());
}

#[cfg(test)]
// Sites below are on values this test constructed from literals it controls.
#[allow(clippy::expect_used)]
mod tests {
    use super::{KeyProvider, Wiring};
    use oqueue_core::{Error, KeyId, Redacted};
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let waker = Waker::noop();
        let mut cx = Context::from_waker(waker);
        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(v) => return v,
                Poll::Pending => std::hint::spin_loop(),
            }
        }
    }

    /// ⚠️ The composition root's only real assertion: the **default** wiring
    /// refuses to wrap a key. A deployment with no KMS configured must fail the
    /// first encryption call, not silently write a plaintext data encryption
    /// key to object storage — ADR-0006. Asserted through the trait object the
    /// shell will actually hold, so a wiring that swapped in an identity
    /// provider would fail here.
    #[test]
    fn the_default_wiring_refuses_to_wrap() {
        let wiring = Wiring::unencrypted();
        let keys: &dyn KeyProvider = wiring.keys.as_ref();
        let id = KeyId::new("arn:aws:kms:eu-west-1:1:key/a").expect("non-empty");
        let dek = Redacted::new(vec![1, 2, 3]);

        let err = block_on(keys.wrap(&id, &dek)).expect_err("the default must refuse");

        assert_eq!(err, Error::EncryptionDisabled);
    }
}
