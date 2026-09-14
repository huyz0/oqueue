"""librdkafka idempotent-producer conformance: a lost ack forces a real
client retry, and the broker must answer with the *original* offset
rather than double-committing — `M11.10`, `ADR-0031` point 2's
transparent-success case, verified against a real client's own retry
machinery rather than a hand-rolled one (`oqueue-broker`'s own
`produce/idempotent.rs` suite already proves the mechanism; this is
where a gap between that and real client wire behaviour would surface,
per doc 06 §6).

Usage: idempotent_conformance.py

⚠️ **Starts its own broker, with its own `advertise` override** — unlike
`rdkafka_roundtrip.py`, which is handed an address. A transparent proxy
in front of a Kafka broker is not actually transparent: a real client
discovers the broker's address to do its *actual* work from the
`Metadata` response, not from `bootstrap.servers`, so a broker
advertising its own bind address would have the client's Produce call
bypass the proxy entirely the moment it looks up the partition leader.
`bin/oqueue serve <bind> [advertise]`'s own second argument is what
lets this script tell the broker to advertise the proxy's address
instead — chosen at proxy start, before the broker exists, which is
why this script owns starting both rather than being handed an
address someone else picked.

⚠️ **The proxy stays up for the whole script, including the later
consumer check** — the broker advertises the proxy's address for its
entire lifetime, so *any* client that talks to this broker, including
the one this script itself uses afterward to verify no duplicate
landed, gets redirected there by `Metadata` too. Shutting the proxy
down between the produce and the verification step would leave that
second client's own `Metadata`-driven connection refused.

A small, frame-aware asyncio TCP proxy sits between the client and the
real broker. It forwards every frame verbatim in both directions until
the *first* Produce request passes client -> broker — at which point
it still forwards that request (so the broker genuinely receives and
commits it) but remembers its correlation id, and once the broker's own
*response* to that exact request arrives, swallows that response frame
instead of relaying it and severs the connection.

⚠️ **Gated on the broker's actual response, not a fixed delay** — an
earlier version of this script forwarded every broker -> client frame
unconditionally and raced a `sleep()` against the connection close, on
the theory that a local broker's response would always lose that race.
It did not: on an in-memory broker the real ack came back in well under
a millisecond, arriving and reaching the client through the unguarded
forwarding loop before the sleep-then-close ever ran, so the client
received its ack normally and never retried at all — the test passed
without exercising deduplication. Waiting for the *matching* response
before acting removes the race entirely: the response's arrival is
itself the proof the broker already committed, and only then is it
withheld.

⚠️ **A closed connection, not a dropped response frame in isolation** —
a still-earlier version tried dropping the response while leaving the
connection open, and found `enable.idempotence`'s ambiguous-retry
handling does not treat "no answer yet on a still-open connection" and
"the connection died" as the same signal: only the second reliably
makes a real client resend with its *original* sequence rather than
advancing to a new one. A closed connection *is* `M10.9`'s own "ack
lost after a durable write" crash point, reached here from outside the
process rather than by a fault-injection fixture.

⚠️ **`enable.idempotence` is set explicitly here, not left at "its own
default"** — `M11.md`'s own opening framing, repeated in `M11.9`'s
commit and this row, asserts librdkafka enables the idempotent path by
default. That premise is wrong for confluent-kafka's librdkafka
(2.15.0, this harness's pinned client): a wire capture of the very
first version of this script showed a `RecordBatch` header carrying
`producer_id`/`producer_epoch`/`base_sequence` all `-1` — the
non-idempotent sentinel — and zero `InitProducerId` calls across four
real connections and two produce attempts. Left at the ambient
default, this test's own "retry" was an ordinary at-least-once resend
racing a real broker's sub-millisecond response time, not idempotent
retry logic, and it landed a genuine second record at offset 1 every
time — the test would have passed against a broker with **no**
dedup mechanism at all, having never exercised one. `rdkafka_roundtrip.py`
carries the same mistaken premise and is corrected separately.
"""

import asyncio
import re
import struct
import subprocess
import sys
import time

import os.path
import sys

# ⚠️ **`reap` before anything that starts a broker.** Importing it installs
# a `SIGTERM` handler and an `atexit` reaper, so a `run_bounded` ceiling
# stops this leg's own `oqueue serve` instead of orphaning it — `M4.61`, and
# `reap.py`'s own doc for what that does and does not cover.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import reap  # noqa: E402  -- must follow the sys.path line above

from confluent_kafka import Consumer, KafkaError, KafkaException, Producer, TopicPartition

TOPIC = "idempotent-conformance"
PAYLOAD = b"once"
PRODUCE_API_KEY = 0


async def read_frame(reader: asyncio.StreamReader) -> bytes:
    """One length-prefixed Kafka frame, length prefix included."""
    header = await reader.readexactly(4)
    (length,) = struct.unpack(">i", header)
    body = await reader.readexactly(length)
    return header + body


class ProduceAckDropper:
    """Forwards every frame both ways until the first Produce request
    crosses client -> broker; from then on, waits for *that specific
    request's* response (matched by correlation id) and swallows it
    instead of relaying it, severing the connection rather than
    forwarding anything further. Every other connection (the consumer
    that verifies no duplicate landed, for one) is forwarded normally
    throughout, since `dropped_once` only ever fires once."""

    def __init__(self, broker_host: str, broker_port: int) -> None:
        self.broker_host = broker_host
        self.broker_port = broker_port
        self.dropped_once = False
        self.dropped_correlation_id: int | None = None

    async def handle(
        self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        broker_reader, broker_writer = await asyncio.open_connection(
            self.broker_host, self.broker_port
        )
        try:
            await asyncio.gather(
                self._client_to_broker(reader, broker_writer),
                self._broker_to_client(broker_reader, writer),
            )
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        finally:
            for w in (writer, broker_writer):
                w.close()

    async def _client_to_broker(
        self, reader: asyncio.StreamReader, broker_writer: asyncio.StreamWriter
    ) -> None:
        while True:
            frame = await read_frame(reader)
            # A request's body is api_key(2) api_version(2) correlation_id(4)...
            (api_key,) = struct.unpack(">h", frame[4:6])
            broker_writer.write(frame)
            await broker_writer.drain()
            if api_key == PRODUCE_API_KEY and not self.dropped_once:
                self.dropped_once = True
                (correlation_id,) = struct.unpack(">i", frame[8:12])
                self.dropped_correlation_id = correlation_id

    async def _broker_to_client(
        self, broker_reader: asyncio.StreamReader, writer: asyncio.StreamWriter
    ) -> None:
        while True:
            frame = await read_frame(broker_reader)
            # A response's body is correlation_id(4)...
            (correlation_id,) = struct.unpack(">i", frame[4:8])
            if (
                self.dropped_correlation_id is not None
                and correlation_id == self.dropped_correlation_id
            ):
                # This *is* the broker's answer to the dropped Produce --
                # its arrival is the proof the broker already committed --
                # but the client must never see it, or there is nothing
                # left to retry.
                self.dropped_correlation_id = None
                raise ConnectionResetError("simulated ack loss")
            writer.write(frame)
            await writer.drain()


def start_broker(advertise_addr: str) -> tuple[subprocess.Popen, str]:
    """Starts `oqueue serve`, bound to a fresh port but advertising
    `advertise_addr` — the proxy's own address, so a real client's
    post-bootstrap connections land on the proxy too."""
    proc = reap.track(subprocess.Popen(
        ["target/debug/oqueue", "serve", "127.0.0.1:0", advertise_addr],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    ))
    deadline = time.time() + 10
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            if proc.poll() is not None:
                raise RuntimeError("oqueue serve exited before listening")
            continue
        match = re.search(r"listening on (\S+)", line)
        if match:
            return proc, match.group(1)
    proc.kill()
    raise RuntimeError("oqueue serve never printed its listening address")


def produce_through(proxy_addr: str) -> int:
    producer = Producer(
        {
            "bootstrap.servers": proxy_addr,
            # ⚠️ Explicit, not ambient — see the module docstring's
            # "not left at its own default" note. Without this, librdkafka
            # never calls InitProducerId at all and every retry is an
            # ordinary at-least-once resend this test cannot tell apart
            # from a real duplicate.
            "enable.idempotence": True,
            # Short enough that a dropped ack is retried well inside this
            # script's own patience; generous enough that a loaded CI box
            # answering slowly is not mistaken for one that never will.
            "message.timeout.ms": 15000,
            "request.timeout.ms": 3000,
            "socket.timeout.ms": 3000,
        }
    )
    delivered: dict[str, int] = {}

    def on_delivery(err, msg) -> None:
        if err is not None:
            raise KafkaException(err)
        delivered["offset"] = msg.offset()

    producer.produce(TOPIC, PAYLOAD, on_delivery=on_delivery)
    remaining = producer.flush(20)
    assert remaining == 0, f"{remaining} messages undelivered"
    assert "offset" in delivered, "no delivery report arrived"
    return delivered["offset"]


def count_records(bootstrap: str) -> int:
    """How many records this topic's partition actually holds — a second,
    independent lens on durability, so a silently duplicated commit
    cannot hide behind whatever offset the delivery report above happened
    to report.

    ⚠️ **`bootstrap` is the address this consumer *starts at*, not
    necessarily where its `Fetch` calls land** — the broker advertises
    the proxy's address for its whole lifetime (this script's own
    docstring explains why), so this consumer is redirected there by its
    own `Metadata` call regardless of which address it names here. The
    proxy has to still be running when this is called.
    """
    consumer = Consumer(
        {
            "bootstrap.servers": bootstrap,
            "group.id": "idempotent-conformance",
            "auto.offset.reset": "earliest",
            "enable.auto.commit": False,
            "enable.partition.eof": True,
        }
    )
    consumer.assign([TopicPartition(TOPIC, 0, 0)])
    count = 0
    deadline = time.time() + 10
    while time.time() < deadline:
        msg = consumer.poll(1.0)
        if msg is None:
            continue
        if msg.error():
            if msg.error().code() == KafkaError._PARTITION_EOF:
                # ⚠️ **Keep polling, not stop** — `rdkafka_roundtrip.py`'s
                # own `consume()` makes the same choice: EOF means
                # "caught up to the watermark as of this poll", not
                # "nothing more will ever arrive". Once at least one
                # record has actually been seen, though, the commit
                # above is already known final — the delivery report
                # confirmed durability before this function was even
                # called — so a second EOF past that point really is
                # the end, and there is nothing left to wait for.
                if count > 0:
                    break
                continue
            raise KafkaException(msg.error())
        count += 1
    consumer.close()
    return count


async def main() -> None:
    # The proxy's own port is chosen before the broker starts, since the
    # broker's `advertise` argument has to name it.
    dropper = ProduceAckDropper("", 0)  # broker_host/port filled in below
    server = await asyncio.start_server(dropper.handle, "127.0.0.1", 0)
    proxy_port = server.sockets[0].getsockname()[1]
    proxy_addr = f"127.0.0.1:{proxy_port}"

    broker_proc, broker_addr = start_broker(proxy_addr)
    broker_host, broker_port_str = broker_addr.rsplit(":", 1)
    dropper.broker_host = broker_host
    dropper.broker_port = int(broker_port_str)

    try:
        async with server:
            serve_task = asyncio.create_task(server.serve_forever())
            try:
                offset = await asyncio.to_thread(produce_through, proxy_addr)
                assert (
                    dropper.dropped_once
                ), "the proxy never saw a Produce request to interrupt"
                assert dropper.dropped_correlation_id is None, (
                    "the proxy never saw the broker's response to the "
                    "dropped Produce -- the ack was never actually "
                    "withheld, so this run proves nothing"
                )
                print(f"DELIVERED offset={offset}")
                assert offset == 0, f"expected the original offset 0, got {offset}"

                # The proxy is still serving here, deliberately — see the
                # module docstring's "stays up for the whole script" note.
                seen = await asyncio.to_thread(count_records, proxy_addr)
            finally:
                serve_task.cancel()

        print(f"RECORDS {seen}")
        assert seen == 1, (
            f"expected exactly one committed record after the retry, saw {seen} "
            "-- a duplicate was applied rather than deduplicated"
        )
        print("DEDUPLICATED OK")
    finally:
        broker_proc.kill()
        broker_proc.wait()


asyncio.run(main())
