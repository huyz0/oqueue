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

mod compose;
mod security;
mod serve;
mod wall_clock;

use oqueue_core::KeyProvider;
use oqueue_crypto::NoOpKeyProvider;
use serve::serve;
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
    store: Arc<dyn compose::Backend>,
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
fn chosen_store() -> Result<(Arc<dyn compose::Backend>, &'static str), oqueue_core::Error> {
    store_for(selection_from(std::env::var("OQUEUE_STORE"))?.as_deref())
}

/// `OQUEUE_STORE`'s three outcomes, with the environment read out of it.
///
/// ⚠️ **`std::env::var(..).ok()` collapses the distinction that matters**, and
/// this function exists because `chosen_store` used to make exactly that
/// collapse: `NotPresent` is an operator declining to name a backend, and
/// `NotUnicode` is a mistake, and `.ok()` maps both to `None` — which
/// [`store_for`] then reads as "nobody asked" and answers with
/// `FakeObjectStore`. So `OQUEUE_STORE=$'s3\xff'` started a broker that
/// acknowledges records into a heap and loses every one at exit, which is the
/// outcome this module's own doc argues against two paragraphs up while the
/// code did it. Measured, not inferred.
///
/// ⚠️ **`bin/oqueue/src/security/sources.rs`'s `from_var` already draws this
/// distinction** and is tested on all three arms; `M4.18` applied it to the
/// five variables it introduced and this is the one composition-root variable
/// that predates it. Filed by that task's own review, built by `M4.30`.
///
/// ⚠️ **It fails *loudly*, which is why it was not folded into `M4.18`**: the
/// banner names `FakeObjectStore` and `serve`'s `durability_warning` says
/// every acknowledged record is lost at exit. A mis-read `OQUEUE_CREDENTIALS`
/// failed silently *open*, which is a different risk class and got the more
/// urgent fix.
///
/// # Errors
///
/// [`oqueue_core::Error::Permanent`] when the variable is set to something
/// that is not valid Unicode — the same answer an unrecognised name gets,
/// because a value this process cannot read is not one it may guess at.
fn selection_from(
    var: Result<String, std::env::VarError>,
) -> Result<Option<String>, oqueue_core::Error> {
    match var {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(oqueue_core::Error::Permanent),
    }
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
) -> Result<(Arc<dyn compose::Backend>, &'static str), oqueue_core::Error> {
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
            // ⚠️ `var_os`, not `var`: the value that *cannot* be decoded is
            // exactly the one an operator most needs echoed back, and
            // `var(..).unwrap_or_default()` prints an empty string for it —
            // naming the variable while hiding what it was set to. `M4.30`.
            // ⚠️ **Lossy *and* `{:?}`, and the first version of this had only
            // the first half.** `clippy::pedantic` refuses `{:?}` on an
            // `OsString`, which is why the value is rendered lossily — but
            // `to_string_lossy` answers a `Cow<str>`, which `{:?}` is fine
            // on, so the escaping was given up for nothing. A value
            // containing a newline then printed a forged second line:
            // review set `OQUEUE_STORE` to `s3\noqueue: using S3Store
            // (durable, encrypted)` and the banner asserted the opposite of
            // what had happened. Escaped, an undecodable byte still shows as
            // U+FFFD, which is the signal the operator needs.
            eprintln!(
                "oqueue: OQUEUE_STORE={:?} could not be used: {error}",
                std::env::var_os("OQUEUE_STORE")
                    .unwrap_or_default()
                    .to_string_lossy()
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
        (Some("serve"), Some(addr)) => {
            let _ = init_telemetry();
            // ⚠️ **Read before the listener binds, and fatal if it cannot
            // be.** `behavior.md` rule 8: a malformed credential, grant or
            // quota source must stop the process, never degrade to running
            // without it — a broker that fell back to "no credentials"
            // because a path was typo'd accepts every client, which is the
            // opposite of what was asked for and looks identical from
            // outside until somebody reads the logs (`M4.18`).
            let security = match security::configured() {
                Ok(security) => security,
                Err(error) => {
                    eprintln!("oqueue: security configuration could not be used: {error}");
                    std::process::exit(1);
                }
            };
            for line in security.describe() {
                eprintln!("{line}");
            }
            serve(&addr, args.next().as_deref(), &wiring, &Arc::new(security));
        }
        (None, _) => run(&wiring),
        (Some(other), _) => {
            eprintln!("unknown argument {other:?}; usage: oqueue [serve <host:port>]");
            std::process::exit(2);
        }
    }
}

fn init_telemetry() -> bool {
    tracing_subscriber::fmt()
        .json()
        .with_target(false)
        .with_current_span(false)
        .with_span_list(false)
        .try_init()
        .is_ok()
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
    use super::{KeyProvider, Wiring, init_telemetry, selection_from, store_for};
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

    #[test]
    fn serving_telemetry_can_be_initialized() {
        assert!(init_telemetry());
        assert!(!init_telemetry());
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

    /// ⚠️ **`M4.30`'s own acceptance criterion: `NotPresent` and `NotUnicode`
    /// are not the same answer.** `std::env::var(..).ok()` made them one, so a
    /// variable set to bytes this process cannot decode selected the in-memory
    /// store — the operator asked for durability, made a typo no shell would
    /// show them, and got a broker that loses every acknowledged record at
    /// exit.
    /// ⚠️ **No `#[cfg(unix)]`, and the first version had one.**
    /// `selection_from` matches the *variant* and never its payload, so the
    /// `OsString` inside it need not be undecodable for the test to mean
    /// what it says — `M4.18`'s own test of the `from_var` this mirrors
    /// (`security/tests/composition.rs`) constructs `NotUnicode` with a
    /// perfectly ordinary string for the same reason. It also excluded the
    /// two portable assertions below. ⚠️ **This said the cfg "would have
    /// been this repository's only gated test", and that was false when
    /// written** — `bin/oqueue/tests/it/startup.rs:43` carries a
    /// `#[cfg(unix)]`, added by this commit's own sibling, and
    /// `oqueue-codec`'s `compress.rs` gates several tests on `gzip` and
    /// `snappy` features. `testing.md` rule 2 is the argument; uniqueness
    /// never was. `M4.55`.
    #[test]
    fn an_undecodable_selection_is_refused_where_an_absent_one_defaults() {
        assert_eq!(
            selection_from(Err(std::env::VarError::NotPresent)).expect("unset is legal"),
            None,
            "nobody asked, so the in-memory store is the honest answer"
        );
        assert_eq!(
            selection_from(Ok("s3".to_owned())).expect("a set value is passed through"),
            Some("s3".to_owned())
        );

        let undecodable = std::ffi::OsString::from("s3");
        assert!(
            selection_from(Err(std::env::VarError::NotUnicode(undecodable))).is_err(),
            "a value this process cannot read is not one it may guess at"
        );
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
