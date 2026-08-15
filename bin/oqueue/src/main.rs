//! The oqueue broker.
//!
//! ⚠️ **This is the composition root, and the only place in the workspace where
//! a concrete implementation is chosen** (FR-50, NFR-51). Every library crate is
//! written against the trait seams in `oqueue-core`; the decision about which
//! `ObjectStore`, which `Clock` and which `KeyProvider` those seams resolve to
//! is made here and nowhere else.
//!
//! ⚠️ **It starts, prints a version, and exits.** There is no broker yet:
//! `M2` brings the wire protocol and `oqueue-broker` the I/O shell that runs
//! it. What exists here is the shape — the place the wiring will go, and the
//! proof that `check-layering.sh` treats this package as a composer.
#![forbid(unsafe_code)]

use oqueue_core::KeyProvider;
use oqueue_crypto::NoOpKeyProvider;

/// The concrete choices this deployment runs with.
///
/// ⚠️ This is the composition root's entire purpose, and today it has one
/// member. `Clock` and `ObjectStore` join it when a real implementation of
/// either exists — `M1` writes the first — and until then choosing between
/// nothing and nothing would be theatre.
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
