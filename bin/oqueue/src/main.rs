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

use oqueue_core::{KeyProvider, ObjectStore};
use oqueue_crypto::NoOpKeyProvider;
use std::sync::Arc;

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
///
/// ⚠️ **`M3.14` is where that condition came true for `ObjectStore`**, exactly
/// as written above: `serve` now hands one to a `Cluster`, so the member is
/// earned rather than anticipated. `Clock` still is not — nothing this wires
/// schedules or expires yet.
struct Wiring {
    keys: Box<dyn KeyProvider>,
    store: Arc<dyn ObjectStore>,
    store_name: &'static str,
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
    fn unencrypted() -> Result<Self, oqueue_core::Error> {
        let (store, store_name) = chosen_store()?;
        Ok(Self {
            keys: Box::new(NoOpKeyProvider::new()),
            store,
            store_name,
        })
    }
}

/// The object store this deployment runs against, and its name for the banner.
///
/// ⚠️ **Named by `OQUEUE_STORE`, never sniffed.** A composition root that
/// guessed from the presence of `AWS_*` would silently write to a different
/// bucket than the operator meant, and silently fall back when a variable was
/// misspelled. An unset value selects the in-memory store, which is what the
/// client harness runs against — ⚠️ **and which is not durable at all**, so
/// `serve` says so on stderr rather than leaving it to the banner alone.
///
/// ⚠️ **An unrecognised value is an error, not the default.** `behavior.md`
/// rule 8: an out-of-range configuration value fails loudly at startup rather
/// than being silently substituted. `OQUEUE_STORE=S3` is a typo, and starting a
/// broker that acknowledges records into a heap because of one is the outcome
/// the rest of this doc argues against. Only *unset* means "nobody asked".
///
/// ⚠️ **Not a configuration story.** One variable with three values is the
/// smallest thing that lets this binary hand a *real* backend to a `Cluster`;
/// a config file, per-bucket settings and validation are an operability
/// milestone's, and `M3` has none.
fn chosen_store() -> Result<(Arc<dyn ObjectStore>, &'static str), oqueue_core::Error> {
    store_for(std::env::var("OQUEUE_STORE").ok().as_deref())
}

/// The selection itself, with the environment read out of it.
///
/// ⚠️ **Split from [`chosen_store`] so it can be tested at all.** Reading an
/// environment variable is process-global state that `std::env::set_var` makes
/// `unsafe` to write and this binary `#![forbid]`s; a function taking the
/// selection is the same logic with the untestable part lifted one level. The
/// half that remains untested is `var()` itself.
///
/// # Errors
///
/// Whatever building the named backend fails with. ⚠️ **An error, never a
/// fallback**: a misconfigured `s3` that quietly became an in-memory store
/// would acknowledge records into a process's heap and lose every one of them
/// at exit, which is the outcome this whole milestone is written against.
fn store_for(
    selection: Option<&str>,
) -> Result<(Arc<dyn ObjectStore>, &'static str), oqueue_core::Error> {
    match selection {
        Some("s3") => Ok((Arc::new(oqueue_store::S3Store::from_env()?), "S3Store")),
        Some("gcs") => Ok((Arc::new(oqueue_store::GcsStore::from_env()?), "GcsStore")),
        // ⚠️ A fake, named one. `M3` builds no durable metadata log either
        // (`M3.md`'s goal note), so a memory store is the honest default for a
        // binary that already cannot survive a restart — and the banner and
        // `serve`'s warning are what keep it from being mistaken for one.
        None => Ok((
            Arc::new(oqueue_core::FakeObjectStore::new()),
            "FakeObjectStore",
        )),
        // ⚠️ Named something nobody recognises: refused, never defaulted.
        Some(_) => Err(oqueue_core::Error::Permanent),
    }
}

fn main() {
    // The composition root's job, in the order it will always happen: choose
    // the concrete implementations, then hand them to the shell that runs them.
    let wiring = match Wiring::unencrypted() {
        Ok(wiring) => wiring,
        Err(error) => {
            eprintln!(
                "oqueue: OQUEUE_STORE={:?} could not be used: {error}",
                std::env::var("OQUEUE_STORE").unwrap_or_default()
            );
            eprintln!(
                "oqueue: valid values are \"s3\" and \"gcs\"; unset means an in-memory store"
            );
            std::process::exit(1);
        }
    };
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
        let (cluster, serving) =
            build_cluster(advertised_host, advertised_port, Arc::clone(&wiring.store)).await?;
        if let Some(warning) = durability_warning(wiring.store_name) {
            eprintln!("{warning}");
        }
        // ⚠️ Spawned *here*, where something watches it — see `build_cluster`.
        let serving = tokio::spawn(serving.run());
        let dispatcher = Arc::new(oqueue_broker::Dispatcher::new(Arc::new(cluster)));
        println!(
            "oqueue {} ({:?}, {})",
            env!("CARGO_PKG_VERSION"),
            wiring.keys(),
            wiring.store_name
        );
        // ⚠️ The line the harness parses; the port is real, not the `:0`
        // the caller may have passed.
        println!("listening on {local}");
        accept_loop(listener, dispatcher, serving, limits).await
    });
    if let Err(error) = result {
        eprintln!("oqueue: serve failed: {error}");
        std::process::exit(1);
    }
}

/// What an operator must hear about the store this process chose, if anything.
///
/// ⚠️ **Its own function because the condition is the thing worth testing.**
/// Inverted, it warns on the durable backends and stays silent on the one that
/// loses every record at exit — which is the failure this warning exists to
/// prevent, and which nothing inside `serve` can assert.
fn durability_warning(store_name: &str) -> Option<&'static str> {
    (store_name == "FakeObjectStore").then_some(
        "oqueue: WARNING -- OQUEUE_STORE is unset, so records are held in memory \
         and every acknowledged record is lost when this process exits. Set \
         OQUEUE_STORE=s3 or =gcs for a durable one.",
    )
}

/// Accepts connections until the listener or the coordinator's loop gives out.
///
/// ⚠️ **Its own function so `serve` stays inside `code-structure.md`'s fifty
/// lines**, and because the race it runs is the interesting part rather than a
/// detail of binding a port.
async fn accept_loop(
    listener: tokio::net::TcpListener,
    dispatcher: Arc<oqueue_broker::Dispatcher>,
    mut serving: tokio::task::JoinHandle<()>,
    limits: oqueue_broker::ConnectionLimits,
) -> std::io::Result<()> {
    loop {
        // A connection's ending is that connection's news alone, and so
        // is a failed accept: ECONNABORTED and EMFILE are transient, and
        // a broker that dies on either takes every healthy client with
        // it. The pause keeps an out-of-descriptors condition from
        // becoming a hot loop.
        // ⚠️ **The coordinator's loop is one of the two things raced.** If
        // it ever finishes — a panic, or its queue closing — this broker
        // can commit nothing, and a process that kept accepting
        // connections would answer every produce `LEADER_NOT_AVAILABLE`
        // forever while looking healthy to a supervisor. `async-concurrency.md`
        // rule 13: observe the task, do not merely start it.
        tokio::select! {
            joined = &mut serving => {
                return Err(std::io::Error::other(match joined {
                    Ok(()) => "the coordinator loop stopped".to_owned(),
                    Err(error) => format!("the coordinator loop failed: {error}"),
                }));
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let handler = Arc::clone(&dispatcher);
                    tokio::spawn(oqueue_broker::serve_connection(stream, handler, limits));
                }
                Err(error) => {
                    eprintln!("oqueue: accept failed: {error}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            },
        }
    }
}

/// Composes the broker: a coordinator over a metadata log, a materialized
/// index, and the chosen object store.
///
/// ⚠️ **The one place the concrete materialization is chosen** (FR-50).
/// `oqueue-index`'s `MemoryIndex` is `M3`'s answer and not the project's — doc
/// 10 #12's disk engine is open — and it is picked here rather than defaulted
/// inside a library, so swapping it is one line in a composition root.
///
/// ⚠️ **`FakeMetadataLog` is the only `MetadataLog` `M3` builds**
/// (`ADR-0020` point 5, doc 10 #12, `M3.md`'s goal note), so this broker's
/// offsets do not survive its process: a restart re-bases at `Offset::ZERO`
/// over objects that already hold those offsets. `roadmap.md`'s deferral table
/// gives the durable engine to `M6`. The warning below is so an operator hears
/// it from the broker rather than from a consumer.
///
/// ⚠️ **The coordinator's loop is returned, not spawned here.** It is spawned
/// by `serve`, which watches its handle — `async-concurrency.md` rule 13 wants
/// an owner that can *observe* the task, and a `JoinHandle` dropped on the
/// floor observes nothing. A loop that panicked would otherwise leave a process
/// that stays up, accepts connections, and answers every produce
/// `LEADER_NOT_AVAILABLE` forever while no supervisor restarts it, because it
/// never exits and says nothing.
async fn build_cluster(
    host: String,
    port: i32,
    store: Arc<dyn ObjectStore>,
) -> std::io::Result<(oqueue_broker::Cluster, oqueue_coordinator::CoordinatorLoop)> {
    let log = Arc::new(oqueue_core::FakeMetadataLog::new());
    let index = Box::new(oqueue_index::MemoryIndex::new());
    let epoch = oqueue_core::CoordinatorEpoch::new(1);
    let (coordinator, serving, reader) = oqueue_coordinator::Coordinator::open(log, index, epoch)
        .await
        .map_err(|error| {
            std::io::Error::other(format!("the coordinator would not open: {error}"))
        })?;
    eprintln!(
        "oqueue: WARNING -- the metadata log is in memory (M6 owns the durable one). \
         Offsets do not survive a restart."
    );
    let cluster = oqueue_broker::Cluster::new(
        host,
        port,
        oqueue_broker::Sequencing::new(coordinator, reader),
        store,
        &oqueue_broker::WriterId::mint(),
    )
    .map_err(|error| std::io::Error::other(format!("the writer identity was refused: {error}")))?;
    Ok((cluster, serving))
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
    println!(
        "oqueue {} ({:?}, {})",
        env!("CARGO_PKG_VERSION"),
        wiring.keys(),
        wiring.store_name
    );
}

#[cfg(test)]
// Sites below are on values this test constructed from literals it controls.
#[allow(clippy::expect_used)]
mod tests {
    use super::{KeyProvider, Wiring, store_for};
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

    /// ⚠️ **A named backend never silently becomes the in-memory one.** The
    /// value of naming `s3` is that the process writes to S3; a selection that
    /// fell back on a misconfiguration would acknowledge records into a heap
    /// and lose every one at exit. So an unbuildable `s3` is an error, and only
    /// an *unrecognised* selection is the fake.
    #[test]
    fn a_named_backend_is_never_quietly_replaced_by_the_in_memory_one() {
        for named in ["s3", "gcs"] {
            // Unconfigured on this machine is an error, which is the other
            // acceptable outcome and the one CI takes.
            let Ok((_, name)) = store_for(Some(named)) else {
                continue;
            };
            assert_ne!(
                name, "FakeObjectStore",
                "{named} must not resolve to a store that loses records at exit"
            );
        }
    }

    /// ⚠️ **A value nobody recognises is a typo, and a typo must not start a
    /// broker.** `OQUEUE_STORE=S3`, `=aws`, or `=s3 ` with the trailing space a
    /// unit file leaves behind would otherwise run on a store that loses every
    /// acknowledged record at exit — silently, because the operator asked for
    /// durability and was given the default.
    #[test]
    fn an_unrecognised_selection_refuses_to_start_rather_than_defaulting() {
        for typo in ["S3", "aws", "s3 ", "nonsense", ""] {
            assert!(
                store_for(Some(typo)).is_err(),
                "{typo:?} must not resolve to any store at all"
            );
        }
    }

    /// ⚠️ **The in-memory store is the one that gets a warning, and it is the
    /// only one.** Inverted, this would stay silent on the configuration that
    /// loses every acknowledged record and shout at the two that do not.
    #[test]
    fn only_the_in_memory_store_warns_about_durability() {
        let warning = super::durability_warning("FakeObjectStore").expect("it must warn");
        assert!(warning.contains("lost when this process exits"));
        assert_eq!(super::durability_warning("S3Store"), None);
        assert_eq!(super::durability_warning("GcsStore"), None);
    }

    /// ⚠️ And *no* selection is the honest default: nobody asked for a
    /// backend, so the in-memory one is what they get, named in the banner.
    #[test]
    fn no_selection_gives_the_in_memory_store() {
        let (_, name) = store_for(None).expect("the fake always builds");
        assert_eq!(name, "FakeObjectStore");
    }

    /// ⚠️ The composition root's only real assertion: the **default** wiring
    /// refuses to wrap a key. A deployment with no KMS configured must fail the
    /// first encryption call, not silently write a plaintext data encryption
    /// key to object storage — ADR-0006. Asserted through the trait object the
    /// shell will actually hold, so a wiring that swapped in an identity
    /// provider would fail here.
    #[test]
    fn the_default_wiring_refuses_to_wrap() {
        let wiring = Wiring::unencrypted().expect("the default store is in memory");
        let keys: &dyn KeyProvider = wiring.keys.as_ref();
        let id = KeyId::new("arn:aws:kms:eu-west-1:1:key/a").expect("non-empty");
        let dek = Redacted::new(vec![1, 2, 3]);

        let err = block_on(keys.wrap(&id, &dek)).expect_err("the default must refuse");

        assert_eq!(err, Error::EncryptionDisabled);
    }
}
