//! Binding a port and serving it: the listener, the accept loop, and the
//! broker they hand connections to.
//!
//! ⚠️ **Split from `main.rs` because choosing and serving are different jobs.**
//! The composition root decides *which* `ObjectStore`, `MetadataLog` and index
//! this deployment runs with; everything here is what happens once those
//! choices are made, and neither half should grow into the other.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use crate::Wiring;
use std::sync::Arc;
use std::time::Duration;

/// Binds `addr` and serves the M2 protocol until killed.
///
/// ⚠️ The advertised identity is the bound address, verbatim — doc 02 §7.2's
/// trap says identity must be decided, and for a single local stub the bound
/// address is the honest choice: it is the one address a client can reach.
pub(crate) fn serve(
    addr: &str,
    advertise: Option<&str>,
    wiring: &Wiring,
    security: &Arc<crate::security::Security>,
    role: crate::ProcessRole,
) {
    // Sized for the harness, not for production: librdkafka's default
    // `message.max.bytes` is 1 MiB, so 16 MiB clears every test frame;
    // 64 in-flight matches its default per-connection pipelining ceiling;
    // and 120 s outlives any harness pause without holding dead peers.
    // ⚠️ **The idle timeout must also outlast a long poll** (`M3.20`): a
    // parked `Fetch` sends nothing, so the connection looks idle for up to
    // `MAX_PARK_MS`. 120 s against 60 s is a 2x margin, and the test below
    // is what keeps it from being reduced by somebody reading only the
    // sentence above.
    let limits = SERVE_LIMITS;
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
        let built = crate::compose::build_cluster(
            advertised_host,
            advertised_port,
            Arc::clone(&wiring.store),
            role.broker_role(),
        )
        .await?;
        let cluster = Arc::new(built.cluster.with_role(role.broker_role()));
        if let Some(warning) = durability_warning(wiring.store_name) {
            eprintln!("{warning}");
        }
        let serving =
            spawn_role_tasks(built.serving, built.retention, built.log, built.lease, role)?;
        println!(
            "oqueue {} ({:?}, {}, role={role:?})",
            env!("CARGO_PKG_VERSION"),
            wiring.keys(),
            wiring.store_name
        );
        // ⚠️ The line the harness parses; the port is real, not the `:0`
        // the caller may have passed.
        println!("listening on {local}");
        accept_loop(listener, cluster, serving, limits, security).await
    });
    if let Err(error) = result {
        eprintln!("oqueue: serve failed: {error}");
        std::process::exit(1);
    }
}

fn spawn_role_tasks(
    serving: Option<oqueue_coordinator::CoordinatorLoop>,
    retention: oqueue_broker::Retention,
    log: oqueue_broker::CurrentLog,
    lease: Option<Arc<oqueue_core::ObjectStoreLease>>,
    role: crate::ProcessRole,
) -> std::io::Result<Option<tokio::task::JoinHandle<()>>> {
    // ⚠️ Spawned here, where the caller watches it — see `build_cluster`. It
    // replays its log first, retrying while the store is unreachable (`M6.10`).
    let serving = serving.map(|serving| {
        tokio::spawn(serving.run_retrying(|| tokio::time::sleep(oqueue_broker::DEGRADED_RETRY)))
    });
    if starts_retention(role) {
        // ⚠️ Retention is combined-mode maintenance until a data-plane has a
        // remote coordinator seam; it writes trim metadata through the local
        // coordinator and must not run in a read-only data-plane process.
        tokio::spawn(retention.run());
    }
    if starts_coordination_cadence(role) {
        // ⚠️ Checkpointing and lease renewal belong to coordination. A stopped
        // cadence only makes the next cold start slower (`M6.4`).
        let Some(lease) = lease else {
            return Err(std::io::Error::other(
                "coordinator role started without its lease",
            ));
        };
        tokio::spawn(oqueue_broker::checkpoints(log, Arc::clone(&lease)));
        tokio::spawn(oqueue_broker::keep_lease(lease));
    }
    Ok(serving)
}

fn starts_retention(role: crate::ProcessRole) -> bool {
    role == crate::ProcessRole::Combined
}

fn starts_coordination_cadence(role: crate::ProcessRole) -> bool {
    role != crate::ProcessRole::DataPlane
}

/// The connection bounds `serve` runs with.
///
/// ⚠️ **A constant so a test can reach it.** The idle timeout has to outlast
/// the longest long poll, and that is a relationship between two numbers in
/// different crates — the sort of thing a comment records and nothing checks.
const SERVE_LIMITS: oqueue_broker::ConnectionLimits = oqueue_broker::ConnectionLimits {
    max_frame: 16 * 1024 * 1024,
    max_in_flight: 64,
    idle_timeout: Duration::from_mins(2),
};

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

/// How long a TLS handshake may take before the connection is dropped.
///
/// ⚠️ **The one hold that precedes every other limit.** `ConnectionLimits`
/// governs a session that exists; this governs getting one at all, and until
/// it completes the peer has authenticated nothing. Generous against a real
/// client on a slow link — `SERVE_LIMITS.idle_timeout` is 120 s for an
/// established connection — and small beside the unbounded wait it replaces.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Serves one accepted connection on its own task, terminating TLS first
/// when this deployment runs it.
///
/// ⚠️ **The handshake happens here, not in the accept loop.** It is a round
/// trip with the client, so doing it before returning to `accept` would let
/// one slow or malicious peer stall every other connection — the same reason
/// the loop does not read a request frame either.
fn spawn_connection(
    stream: tokio::net::TcpStream,
    security: &Arc<crate::security::Security>,
    cluster: Arc<oqueue_broker::Cluster>,
    limits: oqueue_broker::ConnectionLimits,
) {
    // ⚠️ **Only the acceptor is cloned here; the dispatcher is built on the
    // spawned task.** Building it clones the credential set and the grant
    // map, which is work proportional to the configuration — doing it before
    // returning to `accept` would pay that for every TCP connect, including
    // peers that never finish a handshake. Cloning a `TlsAcceptor` is an
    // `Arc` bump. Found by review.
    let acceptor = security.acceptor.clone();
    let security = Arc::clone(security);
    tokio::spawn(async move {
        if let Some(acceptor) = acceptor {
            // ⚠️ **Bounded, because a handshake is the one thing here that
            // runs before any of `serve_connection`'s own limits apply.** A
            // peer that connects and then says nothing holds a socket and a
            // task for as long as it likes: measured, still open at 145 s
            // against TLS where the cleartext path is closed at 120 s by
            // `idle_timeout`. That is an *unauthenticated* hold, so it is
            // the cheapest possible way to exhaust this broker's file
            // descriptors. `async-concurrency.md` rule 4; found by review.
            //
            // ⚠️ A failed or timed-out handshake is one connection's problem
            // and nobody else's: no reply is possible (there is no session to
            // answer in) and it must not be louder than a dropped
            // connection, or a port scanner would fill the log.
            let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream));
            if let Ok(Ok(stream)) = handshake.await {
                // ⚠️ **Built after the handshake, not before it.** Review
                // caught the comment above promising a saving the code did
                // not make: constructing it first meant every silent TCP
                // connection held a config-sized clone of the credentials
                // and grants for the whole `HANDSHAKE_TIMEOUT`, which is the
                // cost that paragraph exists to avoid.
                let handler = Arc::new(security.dispatcher(cluster));
                let _ = oqueue_broker::serve_connection(stream, handler, limits).await;
            }
        } else {
            let handler = Arc::new(security.dispatcher(cluster));
            let _ = oqueue_broker::serve_connection(stream, handler, limits).await;
        }
    });
}

/// Accepts connections until the listener or the coordinator's loop gives out.
///
/// ⚠️ **Its own function so `serve` stays inside `code-structure.md`'s fifty
/// lines**, and because the race it runs is the interesting part rather than a
/// detail of binding a port.
async fn accept_loop(
    listener: tokio::net::TcpListener,
    cluster: Arc<oqueue_broker::Cluster>,
    serving: Option<tokio::task::JoinHandle<()>>,
    limits: oqueue_broker::ConnectionLimits,
    security: &Arc<crate::security::Security>,
) -> std::io::Result<()> {
    // ⚠️ **The one operator surface for `reaped_reads`**, and it exists because
    // a counter nobody can read is a counter nobody will act on. Doc 12 §4.6:
    // a nonzero rate means `M5`'s deletion delay is too short, and the symptom
    // a client sees is `OFFSET_OUT_OF_RANGE` on a partition that was fine a
    // moment ago. `M9` owns the metrics surface; until then this is a line on
    // stderr, printed only when the number moves.
    let mut reported_reaps = 0_u64;
    let mut reap_tick = tokio::time::interval(Duration::from_mins(1));
    reap_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut serving = Box::pin(async move {
        match serving {
            Some(serving) => match serving.await {
                Ok(()) => Err(std::io::Error::other("the coordinator loop stopped")),
                Err(error) => Err(std::io::Error::other(format!(
                    "the coordinator loop failed: {error}"
                ))),
            },
            None => std::future::pending().await,
        }
    });
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
            _ = reap_tick.tick() => {
                let reaped = cluster.reaped_reads();
                if reaped > reported_reaps {
                    eprintln!(
                        "oqueue: WARNING -- {} read(s) found an object the index still \
                         named and object storage had already reaped. A nonzero rate \
                         means the deletion delay is too short; consumers see \
                         OFFSET_OUT_OF_RANGE.",
                        reaped - reported_reaps
                    );
                    reported_reaps = reaped;
                }
            }
            joined = &mut serving => return joined,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    // ⚠️ **A dispatcher per connection, not one shared.** It
                    // carries the session — this client's own commit
                    // watermark, which its next fetch must be at least as
                    // fresh as (hazard H2, `ADR-0023`) — and a session is per
                    // connection because that is the scope a client
                    // understands. Sharing one would let an unrelated client's
                    // produce raise everybody's freshness bar. The `Cluster`
                    // behind it *is* shared, which is where the cost would
                    // have been.
                    spawn_connection(stream, security, Arc::clone(&cluster), limits);
                }
                Err(error) => {
                    eprintln!("oqueue: accept failed: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
        }
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

#[cfg(test)]
mod role_tests;

#[cfg(test)]
// Sites below are on values these tests constructed from literals they control.
#[allow(clippy::expect_used)]
mod tests {
    use super::{
        Duration, SERVE_LIMITS, advertised_identity, starts_coordination_cadence, starts_retention,
    };
    use crate::ProcessRole;
    use std::sync::Arc;

    fn addr(spec: &str) -> std::net::SocketAddr {
        spec.parse().expect("a literal this test wrote")
    }

    /// ⚠️ **With no override the bound address is the identity**, which doc 02
    /// §7.2 says must be a decision rather than an inference — for a single
    /// local broker the address a client can actually reach is the honest one.
    #[test]
    fn with_no_override_the_identity_is_the_address_actually_bound() {
        let (host, port) = advertised_identity(None, addr("127.0.0.1:9092"))
            .expect("the bound address is always usable");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9092);
    }

    /// ⚠️ **The override is used verbatim**, because the harness's capture
    /// proxy needs clients steered through it rather than at the socket this
    /// process bound — a broker that echoed its own address would send every
    /// client past the proxy.
    #[test]
    fn an_override_is_what_clients_are_told_to_use() {
        let (host, port) = advertised_identity(Some("proxy.example:19092"), addr("127.0.0.1:9092"))
            .expect("host:port parses");
        assert_eq!(host, "proxy.example");
        assert_eq!(port, 19092);
    }

    /// ⚠️ **`rsplit_once`, so an IPv6 literal keeps its colons.** Splitting on
    /// the first colon would advertise `[` as the host.
    #[test]
    fn an_ipv6_override_keeps_its_colons() {
        let (host, port) =
            advertised_identity(Some("[::1]:9092"), addr("127.0.0.1:1")).expect("host:port parses");
        assert_eq!(host, "[::1]");
        assert_eq!(port, 9092);
    }

    /// ⚠️ **A malformed override fails at startup rather than advertising
    /// something wrong.** An identity nobody can reach is a broker every
    /// client finds and then cannot talk to.
    #[test]
    fn a_malformed_override_refuses_to_start() {
        for bad in ["no-colon", "host:notaport", "host:", ""] {
            assert!(
                advertised_identity(Some(bad), addr("127.0.0.1:1")).is_err(),
                "{bad:?} must not become an advertised identity"
            );
        }
    }

    #[test]
    fn role_tasks_follow_their_ownership_boundary() {
        assert!(starts_retention(ProcessRole::Combined));
        assert!(!starts_retention(ProcessRole::Coordinator));
        assert!(!starts_retention(ProcessRole::DataPlane));
        assert!(starts_coordination_cadence(ProcessRole::Coordinator));
        assert!(starts_coordination_cadence(ProcessRole::Combined));
        assert!(!starts_coordination_cadence(ProcessRole::DataPlane));
    }

    /// ⚠️ **A long poll must not outlive the connection it is parked on.** A
    /// parked `Fetch` sends nothing, so `read_frames` sees an idle connection
    /// for the whole park; an idle timeout below the park ceiling disconnects
    /// every client that long-polls, right after answering it.
    #[test]
    fn the_idle_timeout_outlasts_the_longest_park() {
        // ⚠️ The other two bounds, pinned here because the comment above
        // `SERVE_LIMITS` argues for specific numbers: 16 MiB clears
        // librdkafka's 1 MiB `message.max.bytes` with room for a batched
        // frame, and 64 matches its default per-connection pipelining.
        assert_eq!(SERVE_LIMITS.max_frame, 16 * 1024 * 1024);
        assert_eq!(SERVE_LIMITS.max_in_flight, 64);
        assert!(
            SERVE_LIMITS.idle_timeout
                > Duration::from_millis(
                    u64::try_from(oqueue_broker::MAX_PARK_MS).expect("a positive ceiling")
                ),
            "an idle timeout below the park ceiling disconnects every long poll"
        );
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

    /// `M10.13`: the worked example. `docs/researches/13-coordinator-recovery.md`
    /// §"Metastable failure via lease expiry" and this file's own comments both
    /// name the failure class — a broker whose coordinator loop has died but
    /// keeps accepting connections, answering every produce
    /// `LEADER_NOT_AVAILABLE` "while looking healthy" — and until this test,
    /// nothing reproduced it.
    ///
    /// ⚠️ **The cheap half, per `M10.md`'s own row: no crate change.**
    /// `bin/oqueue` has no `[lib]`, so `accept_loop` is reachable only from a
    /// `#[cfg(test)] mod` in this file — the simulated harness in `sim`/
    /// `generated.rs` cannot reach it, and extracting a lib to let it in is
    /// the cost `ADR-0027` deferred until this row made it visible. Every type
    /// `build_cluster` composes is a normal (non-dev) dependency already, so
    /// this constructs the same cluster the binary does, with fakes.
    #[tokio::test]
    async fn the_accept_loop_notices_a_dead_coordinator_rather_than_serving_past_it() {
        let store: Arc<dyn crate::compose::Backend> = Arc::new(oqueue_core::FakeObjectStore::new());
        let built = crate::compose::build_cluster(
            "h".to_owned(),
            1,
            store,
            oqueue_broker::NodeRole::Combined,
        )
        .await
        .expect("an empty fixture composes");
        let cluster = built.cluster;
        let serving = tokio::spawn(built.serving.expect("combined role").run());

        // ⚠️ **Aborted, not left to finish on its own** — an idle coordinator
        // loop never returns, so this is the only way to reach the failure
        // this test is for: a *dead* loop, not a quiescent one.
        serving.abort();
        while !serving.is_finished() {
            tokio::task::yield_now().await;
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the OS hands out an ephemeral port");
        let cluster = Arc::new(cluster);

        // ⚠️ **No timeout wrapped around this, deliberately verified rather
        // than assumed safe.** `serving` has already finished, so `select!`'s
        // `joined` arm is ready on the very first poll; `listener.accept()`
        // never resolves because nothing connects, and `reap_tick` is a
        // minute out. A version of `accept_loop` that dropped the `joined`
        // arm — the defect this whole row exists to catch — leaves this
        // *hanging* rather than failing: nothing in `.pre-commit-config.yaml`,
        // `.githooks/`, or `gates.yml` wraps `cargo test` in a timeout, so
        // `check-budget.sh` never runs to report a slow suite — the commit
        // just never returns, which a developer notices by a different route
        // than a red gate. Recorded here so a reader does not credit the
        // budget check with a backstop it does not have.
        let result = super::accept_loop(
            listener,
            cluster,
            Some(serving),
            SERVE_LIMITS,
            &Arc::new(crate::security::Security::cleartext()),
        )
        .await;

        let error = result.expect_err("a dead coordinator loop must end the accept loop");
        // ⚠️ **The task number is not asserted** — `tokio::task::JoinError`'s
        // own `Display` embeds its internal, process-wide spawn counter,
        // which `M4.15a` shifted by one simply by adding an earlier
        // `tokio::spawn` inside `Cluster::new` (the offset-replay task) —
        // this exact string was `"...task 1 was cancelled"` before that and
        // broke on nothing this test itself changed. What the test's own
        // doc actually claims — the reader needs to know it was the
        // *coordinator*, not merely that something failed — only needs the
        // message to name the coordinator and the cause, not tokio's own
        // incidental numbering.
        let message = error.to_string();
        assert!(
            message.starts_with("the coordinator loop failed: task "),
            "the reader needs to know it was the coordinator, not merely \
             that something failed: {message:?}"
        );
        assert!(
            message.ends_with(" was cancelled"),
            "the reader needs to know *why* the coordinator loop ended: {message:?}"
        );
    }
}
