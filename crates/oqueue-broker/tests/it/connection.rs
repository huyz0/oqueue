//! The connection task's contract: pipelined handling, in-order writes,
//! bounded in-flight, loud endings.

// Test-only: every expect is on machinery the test itself constructed.
#![allow(clippy::expect_used)]

use oqueue_broker::{ConnectionEnd, ConnectionLimits, serve_connection};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

const LIMITS: ConnectionLimits = ConnectionLimits {
    max_frame: 1024,
    max_in_flight: 8,
    idle_timeout: std::time::Duration::from_hours(1),
};

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&i32::try_from(body.len()).expect("small").to_be_bytes());
    out.extend_from_slice(body);
    out
}

async fn read_response(client: &mut (impl AsyncReadExt + Unpin)) -> Vec<u8> {
    let mut size = [0u8; 4];
    client.read_exact(&mut size).await.expect("a frame size");
    let mut body = vec![0u8; u32::from_be_bytes(size) as usize];
    client.read_exact(&mut body).await.expect("a frame body");
    body
}

/// The load-bearing property: request 0's handler is *held* until request
/// 1's handler has finished, yet the responses still arrive 0 then 1 —
/// out-of-order completion, in-order writes, no timing involved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_stay_in_request_order_despite_reversed_completion() {
    let (mut client, server) = tokio::io::duplex(4096);
    let release_first = Arc::new(Notify::new());
    let second_done = Arc::new(Notify::new());

    let handler = {
        let release_first = Arc::clone(&release_first);
        let second_done = Arc::clone(&second_done);
        move |req: Vec<u8>| {
            let release_first = Arc::clone(&release_first);
            let second_done = Arc::clone(&second_done);
            async move {
                if req == b"first" {
                    release_first.notified().await;
                } else {
                    second_done.notify_one();
                }
                req
            }
        }
    };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), LIMITS));

    client.write_all(&frame(b"first")).await.expect("write");
    client.write_all(&frame(b"second")).await.expect("write");
    // Only release the first handler after the second has provably finished.
    second_done.notified().await;
    release_first.notify_one();

    assert_eq!(read_response(&mut client).await, b"first");
    assert_eq!(read_response(&mut client).await, b"second");
    drop(client);
    conn.await
        .expect("task joins")
        .expect("a clean close after the peer drops");
}

/// The read half stops reading at `max_in_flight`: with two permitted and
/// every handler parked, a third request is never started.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_in_flight_bound_stops_the_read_half() {
    let (mut client, server) = tokio::io::duplex(4096);
    let started = Arc::new(AtomicUsize::new(0));
    // Zero permits parks every handler; the release adds enough for all,
    // and stays open for handlers that start later.
    let park = Arc::new(tokio::sync::Semaphore::new(0));
    let observed_two = Arc::new(Notify::new());

    let handler = {
        let started = Arc::clone(&started);
        let park = Arc::clone(&park);
        let observed_two = Arc::clone(&observed_two);
        move |req: Vec<u8>| {
            let started = Arc::clone(&started);
            let park = Arc::clone(&park);
            let observed_two = Arc::clone(&observed_two);
            async move {
                if started.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
                    observed_two.notify_one();
                }
                let _permit = park.acquire().await.expect("never closed");
                req
            }
        }
    };
    let limits = ConnectionLimits {
        max_frame: 1024,
        max_in_flight: 2,
        idle_timeout: std::time::Duration::from_hours(1),
    };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), limits));

    for body in [&b"a"[..], b"b", b"c"] {
        client.write_all(&frame(body)).await.expect("write");
    }
    observed_two.notified().await;
    // Both permits are held and parked; the third frame sits unread. Yield
    // generously so a wrong implementation would have started it.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert_eq!(started.load(Ordering::SeqCst), 2, "the third never started");

    // Open the gate for everyone, including the not-yet-started third.
    park.add_permits(64);
    assert_eq!(read_response(&mut client).await, b"a");
    assert_eq!(read_response(&mut client).await, b"b");
    assert_eq!(read_response(&mut client).await, b"c");
    drop(client);
    conn.await.expect("joins").expect("clean close");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_over_the_cap_ends_the_connection_loudly() {
    let (mut client, server) = tokio::io::duplex(4096);
    let handler = |req: Vec<u8>| async move { req };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), LIMITS));
    client
        .write_all(&2048i32.to_be_bytes())
        .await
        .expect("write");
    let end = conn.await.expect("joins").expect_err("the cap trips");
    assert!(matches!(
        end,
        ConnectionEnd::FrameTooLarge { declared: 2048 }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_negative_frame_size_ends_the_connection_loudly() {
    let (mut client, server) = tokio::io::duplex(4096);
    let handler = |req: Vec<u8>| async move { req };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), LIMITS));
    client
        .write_all(&(-5i32).to_be_bytes())
        .await
        .expect("write");
    let end = conn.await.expect("joins").expect_err("negative refused");
    assert!(matches!(
        end,
        ConnectionEnd::NegativeFrameSize { declared: -5 }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_handler_is_this_connections_error_alone() {
    let (mut client, server) = tokio::io::duplex(4096);
    let handler = |_req: Vec<u8>| async move { panic!("handler bug") };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), LIMITS));
    client.write_all(&frame(b"boom")).await.expect("write");
    let end = conn.await.expect("the serve task itself survives");
    assert!(matches!(end, Err(ConnectionEnd::HandlerPanicked)));
}

/// A peer dying mid-frame is an I/O ending, not a clean close and not a
/// hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_dying_mid_frame_is_an_io_ending() {
    let (mut client, server) = tokio::io::duplex(4096);
    let handler = |req: Vec<u8>| async move { req };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), LIMITS));
    client.write_all(&10i32.to_be_bytes()).await.expect("size");
    client.write_all(b"abc").await.expect("partial body");
    drop(client);
    let end = conn.await.expect("joins").expect_err("mid-frame death");
    assert!(matches!(end, ConnectionEnd::Io(_)));
}

/// The endings say what happened, in words an operator can act on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_ending_displays_its_facts() {
    assert_eq!(
        ConnectionEnd::FrameTooLarge { declared: 9 }.to_string(),
        "peer declared a 9-byte frame over the cap"
    );
    assert_eq!(
        ConnectionEnd::NegativeFrameSize { declared: -1 }.to_string(),
        "peer declared a negative frame size -1"
    );
    assert_eq!(
        ConnectionEnd::HandlerPanicked.to_string(),
        "a request handler panicked"
    );
    assert_eq!(
        ConnectionEnd::TimedOut {
            waiting_for: "frame"
        }
        .to_string(),
        "peer idle past the limit while waiting for frame"
    );
    assert_eq!(
        ConnectionEnd::ConnectionTaskFailed.to_string(),
        "a connection task died — a broker bug, not the peer's"
    );
    assert!(
        ConnectionEnd::Io(std::io::Error::other("x"))
            .to_string()
            .starts_with("socket error:")
    );
}

/// A peer that goes quiet mid-frame ends on the idle timeout, not never —
/// paused time makes the hour pass instantly and deterministically.
#[tokio::test(start_paused = true)]
async fn a_silent_peer_times_out_deterministically() {
    let (mut client, server) = tokio::io::duplex(4096);
    let handler = |req: Vec<u8>| async move { req };
    let limits = ConnectionLimits {
        max_frame: 1024,
        max_in_flight: 2,
        idle_timeout: std::time::Duration::from_secs(30),
    };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), limits));
    // Half a frame size, then silence. Paused time auto-advances when every
    // task is idle, so the timeout fires without a real wait.
    client.write_all(&[0u8, 0]).await.expect("partial size");
    let end = conn
        .await
        .expect("joins")
        .expect_err("the idle timeout fires");
    assert!(matches!(
        end,
        ConnectionEnd::TimedOut {
            waiting_for: "frame"
        }
    ));
}

/// A peer that reads nothing while responses pile up ends on the write
/// timeout rather than wedging the writer forever.
#[tokio::test(start_paused = true)]
async fn a_peer_that_never_reads_times_out_the_write() {
    // A tiny pipe fills fast; the client never reads.
    let (mut client, server) = tokio::io::duplex(8);
    let handler = |req: Vec<u8>| async move { req.repeat(16) };
    let limits = ConnectionLimits {
        max_frame: 1024,
        max_in_flight: 2,
        idle_timeout: std::time::Duration::from_secs(30),
    };
    let conn = tokio::spawn(serve_connection(server, Arc::new(handler), limits));
    client.write_all(&frame(b"stuffed")).await.expect("write");
    let end = conn.await.expect("joins").expect_err("the write wedges");
    // Both halves are stuck against this peer — the writer on the full pipe
    // and the reader on the next frame — and both timers fire; which one
    // surfaces is the select's pick. The property is that the connection
    // *ends on a timeout* instead of wedging; the write path's own timer is
    // exercised either way.
    assert!(matches!(end, ConnectionEnd::TimedOut { .. }));
}
