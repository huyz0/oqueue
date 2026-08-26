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
use oqueue_core::ObjectStore;
use std::sync::Arc;

/// Binds `addr` and serves the M2 protocol until killed.
///
/// ⚠️ The advertised identity is the bound address, verbatim — doc 02 §7.2's
/// trap says identity must be decided, and for a single local stub the bound
/// address is the honest choice: it is the one address a client can reach.
pub(crate) fn serve(addr: &str, advertise: Option<&str>, wiring: &Wiring) {
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
        let (cluster, serving) =
            build_cluster(advertised_host, advertised_port, Arc::clone(&wiring.store)).await?;
        if let Some(warning) = durability_warning(wiring.store_name) {
            eprintln!("{warning}");
        }
        // ⚠️ Spawned *here*, where something watches it — see `build_cluster`.
        let serving = tokio::spawn(serving.run());
        let cluster = Arc::new(cluster);
        println!(
            "oqueue {} ({:?}, {})",
            env!("CARGO_PKG_VERSION"),
            wiring.keys(),
            wiring.store_name
        );
        // ⚠️ The line the harness parses; the port is real, not the `:0`
        // the caller may have passed.
        println!("listening on {local}");
        accept_loop(listener, cluster, serving, limits).await
    });
    if let Err(error) = result {
        eprintln!("oqueue: serve failed: {error}");
        std::process::exit(1);
    }
}

/// The connection bounds `serve` runs with.
///
/// ⚠️ **A constant so a test can reach it.** The idle timeout has to outlast
/// the longest long poll, and that is a relationship between two numbers in
/// different crates — the sort of thing a comment records and nothing checks.
const SERVE_LIMITS: oqueue_broker::ConnectionLimits = oqueue_broker::ConnectionLimits {
    max_frame: 16 * 1024 * 1024,
    max_in_flight: 64,
    idle_timeout: std::time::Duration::from_mins(2),
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

/// Accepts connections until the listener or the coordinator's loop gives out.
///
/// ⚠️ **Its own function so `serve` stays inside `code-structure.md`'s fifty
/// lines**, and because the race it runs is the interesting part rather than a
/// detail of binding a port.
async fn accept_loop(
    listener: tokio::net::TcpListener,
    cluster: Arc<oqueue_broker::Cluster>,
    mut serving: tokio::task::JoinHandle<()>,
    limits: oqueue_broker::ConnectionLimits,
) -> std::io::Result<()> {
    // ⚠️ **The one operator surface for `reaped_reads`**, and it exists because
    // a counter nobody can read is a counter nobody will act on. Doc 12 §4.6:
    // a nonzero rate means `M5`'s deletion delay is too short, and the symptom
    // a client sees is `OFFSET_OUT_OF_RANGE` on a partition that was fine a
    // moment ago. `M9` owns the metrics surface; until then this is a line on
    // stderr, printed only when the number moves.
    let mut reported_reaps = 0_u64;
    let mut reap_tick = tokio::time::interval(std::time::Duration::from_mins(1));
    reap_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
            joined = &mut serving => {
                return Err(std::io::Error::other(match joined {
                    Ok(()) => "the coordinator loop stopped".to_owned(),
                    Err(error) => format!("the coordinator loop failed: {error}"),
                }));
            }
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
                    let handler = Arc::new(oqueue_broker::Dispatcher::new(Arc::clone(&cluster)));
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

#[cfg(test)]
// Sites below are on values these tests constructed from literals they control.
#[allow(clippy::expect_used)]
mod tests {
    use super::{SERVE_LIMITS, advertised_identity};

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
                > std::time::Duration::from_millis(
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
}
