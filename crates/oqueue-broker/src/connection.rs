//! One client connection: framed reads, pipelined handling, in-order writes.
//!
//! ⚠️ **The protocol guarantees per-connection response order.** Kafka's
//! spec promises that responses return in request order on a TCP
//! connection, and the Java client asserts it — `M2.md`'s "out-of-order
//! responses" plan line described what *handlers* may do, not the wire. So
//! the shape is: the read half keeps accepting frames (pipelining), each
//! request's handler runs as its own task (out-of-order completion), and a
//! sequencer holds completed responses until their turn (in-order writes).
//!
//! `async-concurrency.md`, applied: every channel is bounded and the
//! in-flight semaphore is the backpressure (rules 4, 7); no lock anywhere,
//! shared state is messages (rule 7); each handler task's `JoinHandle` is
//! observed by the sequencer, so a panic surfaces as this connection's
//! error and no other's (rules 13, 15). ⚠️ **Dropping the returned future
//! aborts the reader and writer tasks** — they are held in abort-on-drop
//! guards, because a dropped `JoinHandle` *detaches* a task, and the first
//! draft's claim that child tasks die with the future was a measured lie
//! (rule 10; found by review). In-flight handler tasks are not aborted:
//! they hold no socket and no external resource, so they run out and their
//! results are dropped — stated rather than hidden. Every peer-facing
//! await wears the idle timeout (rule 4): a peer that stops mid-frame or
//! stops reading responses ends the connection, never wedges it.

use oqueue_codec::frame::write_frame;
use std::future::Future;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Semaphore, mpsc};

/// Per-connection bounds — every one client-facing, every one a constant
/// the composition root passes in, never an environment read.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionLimits {
    /// Largest frame body accepted, in bytes. A peer declaring more is a
    /// protocol error and the connection ends.
    pub max_frame: u32,
    /// Requests allowed in flight (read but unanswered) per connection —
    /// the backpressure bound: the read half stops reading at the limit.
    pub max_in_flight: usize,
    /// The longest any single peer-facing wait may take — between frames,
    /// mid-frame, or writing a response. Kafka's own
    /// `connections.max.idle.ms` shape; a slowloris peer ends here.
    pub idle_timeout: std::time::Duration,
}

/// A handler's verdict on one request (ADR-0018's status notes).
#[derive(Debug, PartialEq, Eq)]
pub enum HandlerResponse {
    /// Write this complete response body (header included; framing is the
    /// connection's).
    Reply(Vec<u8>),
    /// Write nothing and keep going — `acks=0`'s fire-and-forget, where a
    /// reply would desync the client's read stream (`M2.23`).
    Silent,
    /// Close the connection — the only safe answer to a request no
    /// response schema fits.
    Close,
}

/// What a connection does with one decoded frame.
///
/// The dispatcher `M2.21` builds; the verdict shapes are
/// [`HandlerResponse`]'s.
pub trait Handler: Send + Sync + 'static {
    /// The response future for one request frame.
    fn handle(&self, request: Vec<u8>) -> impl Future<Output = HandlerResponse> + Send;
}

impl<F, Fut> Handler for F
where
    F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = HandlerResponse> + Send,
{
    fn handle(&self, request: Vec<u8>) -> impl Future<Output = HandlerResponse> + Send {
        self(request)
    }
}

/// Why a connection ended — the clean close is `serve_connection`'s `Ok`,
/// not a variant here.
#[derive(Debug)]
pub enum ConnectionEnd {
    /// A peer-facing wait outlived [`ConnectionLimits::idle_timeout`].
    TimedOut {
        /// Which wait: "frame", "body", or "write".
        waiting_for: &'static str,
    },
    /// The peer declared a frame over [`ConnectionLimits::max_frame`].
    FrameTooLarge {
        /// The declared size.
        declared: u32,
    },
    /// The peer sent a negative frame size.
    NegativeFrameSize {
        /// The declared size.
        declared: i32,
    },
    /// The socket failed mid-frame or mid-write.
    Io(std::io::Error),
    /// A handler task panicked — this connection's failure, nobody else's
    /// (`async-concurrency.md` rule 15); the panic payload is in the log,
    /// not here.
    HandlerPanicked,
    /// The reader or writer task itself died — a broker bug, distinct from
    /// a handler's panic so an operator hunts in the right place.
    ConnectionTaskFailed,
}

impl core::fmt::Display for ConnectionEnd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TimedOut { waiting_for } => {
                write!(
                    f,
                    "peer idle past the limit while waiting for {waiting_for}"
                )
            }
            Self::FrameTooLarge { declared } => {
                write!(f, "peer declared a {declared}-byte frame over the cap")
            }
            Self::NegativeFrameSize { declared } => {
                write!(f, "peer declared a negative frame size {declared}")
            }
            Self::Io(e) => write!(f, "socket error: {e}"),
            Self::HandlerPanicked => write!(f, "a request handler panicked"),
            Self::ConnectionTaskFailed => {
                write!(f, "a connection task died — a broker bug, not the peer's")
            }
        }
    }
}

/// Serves one connection until the peer closes, a bound trips, or the
/// socket fails. Generic over the stream so tests drive it with
/// `tokio::io::duplex` and no real socket exists below `bin/oqueue`.
///
/// # Errors
/// Every ending except a clean close — see [`ConnectionEnd`].
pub async fn serve_connection<S, H>(
    socket: S,
    handler: Arc<H>,
    limits: ConnectionLimits,
) -> Result<(), ConnectionEnd>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
    H: Handler,
{
    let (read_half, write_half) = tokio::io::split(socket);
    // Completed responses, sequenced: capacity ties memory to the in-flight
    // bound rather than to the peer's appetite.
    let (done_tx, done_rx) = mpsc::channel::<(u64, tokio::task::JoinHandle<HandlerResponse>)>(
        limits.max_in_flight.max(1),
    );
    // Abort-on-drop guards: cancelling `serve_connection` genuinely ends
    // both tasks (rule 10) instead of detaching them against a live socket.
    let mut writer = AbortOnDrop(tokio::spawn(write_in_order(
        write_half,
        done_rx,
        limits.idle_timeout,
    )));
    // The read loop is its own task and **owns** `done_tx`: when it ends —
    // for any reason — the channel closes, which is the writer's shutdown
    // signal (rule 14), and the writer drains what completed and exits.
    let mut reader = AbortOnDrop(tokio::spawn(read_frames(
        read_half, handler, limits, done_tx,
    )));

    // ⚠️ The writer dying must end the reads too, or a panicked handler
    // leaves the connection wedged against a still-open peer — found by the
    // panic test hanging. Whichever side finishes first decides.
    tokio::select! {
        r = &mut reader.0 => {
            let read_result = r.map_err(|_| ConnectionEnd::ConnectionTaskFailed)?;
            let write_result = (&mut writer.0)
                .await
                .map_err(|_| ConnectionEnd::ConnectionTaskFailed)?;
            read_result?;
            write_result.map(|_| ())
        }
        w = &mut writer.0 => {
            match w.map_err(|_| ConnectionEnd::ConnectionTaskFailed)? {
                // ⚠️ A drained exit means the channel closed, which only the
                // reader ending does — so the reader has a verdict and it,
                // not this Ok, is the connection's. Found live: `select!`
                // picks arms at random when both tasks finish in one poll
                // gap, and this arm was swallowing the reader's timeout as
                // a clean close.
                Ok(WriterEnd::Drained) => (&mut reader.0)
                    .await
                    .map_err(|_| ConnectionEnd::ConnectionTaskFailed)?,
                // A handler chose to close: the socket is already shut
                // down; end the reads now rather than let them idle out
                // blaming the peer (round 1's second live find).
                Ok(WriterEnd::HandlerClosed) => {
                    reader.0.abort();
                    Ok(())
                }
                // A writer error is the connection's end; stop reading into
                // a dead pipe (rule 13: this is the owner observing).
                Err(e) => {
                    reader.0.abort();
                    Err(e)
                }
            }
        }
    }
}

/// A task that dies with its owner — dropping this aborts the task, which
/// is what makes `serve_connection` cancellation-correct.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The read half: frames in, handlers spawned, sequence numbers out.
async fn read_frames<S, H>(
    mut read_half: tokio::io::ReadHalf<S>,
    handler: Arc<H>,
    limits: ConnectionLimits,
    done_tx: mpsc::Sender<(u64, tokio::task::JoinHandle<HandlerResponse>)>,
) -> Result<(), ConnectionEnd>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
    H: Handler,
{
    let in_flight = Arc::new(Semaphore::new(limits.max_in_flight.max(1)));
    let mut sequence: u64 = 0;
    loop {
        let mut size_buf = [0u8; 4];
        let size_read =
            tokio::time::timeout(limits.idle_timeout, read_half.read_exact(&mut size_buf))
                .await
                .map_err(|_| ConnectionEnd::TimedOut {
                    waiting_for: "frame",
                })?;
        match size_read {
            Ok(_) => {}
            // A clean close lands here as EOF on the size read.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(ConnectionEnd::Io(e)),
        }
        let declared = i32::from_be_bytes(size_buf);
        let Ok(size) = u32::try_from(declared) else {
            return Err(ConnectionEnd::NegativeFrameSize { declared });
        };
        if size > limits.max_frame {
            return Err(ConnectionEnd::FrameTooLarge { declared: size });
        }
        // The bound checked above is what makes this allocation safe.
        let mut body = vec![0u8; size as usize];
        tokio::time::timeout(limits.idle_timeout, read_half.read_exact(&mut body))
            .await
            .map_err(|_| ConnectionEnd::TimedOut {
                waiting_for: "body",
            })?
            .map_err(ConnectionEnd::Io)?;

        // Backpressure: no permit, no further reads. The permit rides
        // inside the handler task and frees on completion. Acquire fails
        // only on a closed semaphore, and nothing closes this one.
        let Ok(permit) = Arc::clone(&in_flight).acquire_owned().await else {
            return Ok(());
        };
        let h = Arc::clone(&handler);
        let task = tokio::spawn(async move {
            let _permit = permit;
            h.handle(body).await
        });
        if done_tx.send((sequence, task)).await.is_err() {
            // Writer gone; serve_connection's select surfaces its error.
            return Ok(());
        }
        sequence += 1;
    }
}

/// The write half: joins each handler in sequence order and writes its
/// frame. Sequential joining *is* the reordering — task `n+1` may finish
/// first, but its bytes wait until `n`'s are on the wire.
/// How the writer finished: drained after the reader ended, or a handler
/// decided to close the connection.
enum WriterEnd {
    Drained,
    HandlerClosed,
}

async fn write_in_order<W: AsyncWrite + Send + Unpin>(
    mut write_half: W,
    mut done_rx: mpsc::Receiver<(u64, tokio::task::JoinHandle<HandlerResponse>)>,
    idle_timeout: std::time::Duration,
) -> Result<WriterEnd, ConnectionEnd> {
    let mut expected: u64 = 0;
    while let Some((sequence, task)) = done_rx.recv().await {
        debug_assert_eq!(
            sequence, expected,
            "the reader hands out sequences in order"
        );
        expected = expected.wrapping_add(1);
        let response = match task.await.map_err(|_| ConnectionEnd::HandlerPanicked)? {
            HandlerResponse::Reply(bytes) => bytes,
            // Fire-and-forget: nothing on the wire, sequencing intact.
            HandlerResponse::Silent => continue,
            HandlerResponse::Close => {
                // The handler's close decision: everything already written
                // stands, nothing more is, and the socket is shut down NOW —
                // review found the first draft leaving the reader parked
                // against a peer waiting for a reply that never comes.
                write_half.flush().await.map_err(ConnectionEnd::Io)?;
                let _ = write_half.shutdown().await;
                return Ok(WriterEnd::HandlerClosed);
            }
        };
        let mut frame = Vec::with_capacity(response.len() + 4);
        if write_frame(&mut frame, &response).is_err() {
            // A response over i32::MAX is a broker bug, not a peer error;
            // refuse loudly rather than send a wrapped length.
            return Err(ConnectionEnd::Io(std::io::Error::other(
                "response body over i32::MAX",
            )));
        }
        tokio::time::timeout(idle_timeout, write_half.write_all(&frame))
            .await
            .map_err(|_| ConnectionEnd::TimedOut {
                waiting_for: "write",
            })?
            .map_err(ConnectionEnd::Io)?;
    }
    write_half.flush().await.map_err(ConnectionEnd::Io)?;
    Ok(WriterEnd::Drained)
}
