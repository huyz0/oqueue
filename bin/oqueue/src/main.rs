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
//! ⚠️ ~~It starts, prints a version, and exits.~~ — **`serve` exists now**
//! (`M2.25`): `oqueue serve <host:port>` binds a listener and serves the M2
//! wire protocol over a stub partition. With no arguments the banner-and-exit
//! behaviour stands, byte-pinned by `tests/it/startup.rs`.
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
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        // `oqueue serve <addr> [advertise]` -- the M2 broker: the wire
        // protocol over a stub partition. `<addr>` may end in `:0` to let
        // the OS pick; the printed line names the port actually bound,
        // which is what the client harness parses. `advertise` overrides
        // the identity Metadata hands out (doc 02 §7.2: identity is a
        // decision) -- the harness's capture proxy needs clients steered
        // through it rather than at the socket this process bound.
        (Some("serve"), Some(addr)) => serve(&addr, args.next().as_deref(), &wiring),
        (None, _) => run(&wiring),
        (Some(other), _) => {
            eprintln!("unknown argument {other:?}; usage: oqueue [serve <host:port>]");
            std::process::exit(2);
        }
    }
}

/// Binds `addr` and serves the M2 protocol until killed.
///
/// ⚠️ The advertised identity is the bound address, verbatim — doc 02 §7.2's
/// trap says identity must be decided, and for a single local stub the bound
/// address is the honest choice: it is the one address a client can reach.
fn serve(addr: &str, advertise: Option<&str>, wiring: &Wiring) {
    // Sized for the harness, not for production: librdkafka's default
    // `message.max.bytes` is 1 MiB, so 16 MiB clears every test frame;
    // 64 in-flight matches its default per-connection pipelining ceiling;
    // and 120 s outlives any harness pause without holding dead peers.
    let limits = oqueue_broker::ConnectionLimits {
        max_frame: 16 * 1024 * 1024,
        max_in_flight: 64,
        idle_timeout: std::time::Duration::from_mins(2),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("oqueue: runtime failed to start: {error}");
            std::process::exit(1);
        }
    };
    let result: std::io::Result<()> = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let (advertised_host, advertised_port) = advertised_identity(advertise, local)?;
        let cluster = std::sync::Arc::new(oqueue_broker::StubCluster::new(
            advertised_host,
            advertised_port,
        ));
        let dispatcher = std::sync::Arc::new(oqueue_broker::Dispatcher::new(cluster));
        println!("oqueue {} ({:?})", env!("CARGO_PKG_VERSION"), wiring.keys());
        // ⚠️ The line the harness parses; the port is real, not the `:0`
        // the caller may have passed.
        println!("listening on {local}");
        loop {
            // A connection's ending is that connection's news alone, and so
            // is a failed accept: ECONNABORTED and EMFILE are transient, and
            // a broker that dies on either takes every healthy client with
            // it. The pause keeps an out-of-descriptors condition from
            // becoming a hot loop.
            match listener.accept().await {
                Ok((stream, _)) => {
                    let handler = std::sync::Arc::clone(&dispatcher);
                    tokio::spawn(oqueue_broker::serve_connection(stream, handler, limits));
                }
                Err(error) => {
                    eprintln!("oqueue: accept failed: {error}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    });
    if let Err(error) = result {
        eprintln!("oqueue: serve failed: {error}");
        std::process::exit(1);
    }
}

/// The advertised identity: the override when given, else the bound
/// address — the one address a client can reach a local stub at.
fn advertised_identity(
    advertise: Option<&str>,
    local: std::net::SocketAddr,
) -> std::io::Result<(String, i32)> {
    let Some(spec) = advertise else {
        return Ok((local.ip().to_string(), i32::from(local.port())));
    };
    spec.rsplit_once(':')
        .and_then(|(host, port)| Some((host.to_owned(), port.parse::<i32>().ok()?)))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "advertise must be host:port",
            )
        })
}

/// The no-argument behaviour: the banner, and nothing else.
///
/// ⚠️ ~~There is no broker.~~ — `serve` above runs one (`M2.25`). This stays
/// the default so that `oqueue` in a terminal never silently binds a port.
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
