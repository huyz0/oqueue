"""Do committed offsets survive a broker restart? (`M4.17`, FR-21.)

Usage: offset_survival.py

⚠️ **This leg reports what it measures and does not assert FR-21 today.**
FR-21's own verification method in `requirements.md` is "offsets survive a
full broker fleet restart", and they do not: `bin/oqueue serve` wires
`FakeGroupMetadataLog`, which holds its entries in an in-memory `Vec`, so a
process restart loses every committed offset. That is a recorded deferral,
not a surprise — `ADR-0035` says the real group-metadata-log engine is `M6`'s
work and `roadmap.md`'s "Deferred into a later milestone" table receives it —
and `serve` says so on stderr at startup.

⚠️ **So it prints one of two verdicts, and both are useful.** `OFFSETS LOST`
is today's expected result and the harness reports it as a *skip* with this
reason, never as a pass: a green harness must not be readable as FR-21 being
verified. `OFFSETS SURVIVED` means the durable engine landed and this leg
should be promoted to an assertion — the same check-it-in-both-directions
discipline `AGENTS.md` uses for its own "every script exists" paragraph, so
that the good news cannot pass unnoticed either.

⚠️ **Its own brokers, not the harness's `$ADDR`.** Restarting is the whole
point, so this script owns both lifecycles.
"""

import queue
import re
import subprocess
import sys
import threading
import time

from confluent_kafka import Consumer, Producer, TopicPartition

TOPIC = "offset-survival"
GROUP = "offset-survival-group"
RECORDS = 5


def start_broker() -> tuple[subprocess.Popen, str]:
    """Starts `oqueue serve` and returns it with the address it printed.

    ⚠️ **The reader runs on its own thread, because `readline()` blocks and
    would defeat the deadline below.** A broker that is alive but silent —
    started, not yet listening, and never going to — would otherwise park
    this call inside `readline()` forever, and with it the harness and both
    gates that invoke the harness. The deadline has to be enforced by
    something the read cannot block. Found by review.
    """
    proc = subprocess.Popen(
        ["target/debug/oqueue", "serve", "127.0.0.1:0"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: queue.Queue = queue.Queue()

    def pump() -> None:
        for line in proc.stdout:
            lines.put(line)
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()

    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            # ⚠️ `max(0.0, ...)`: the clock is re-read after the `while`
            # test, so a thread switch at the deadline yields a negative
            # timeout and `queue.get` raises `ValueError` — which would skip
            # the `proc.kill()` below and orphan the broker. Found by review.
            line = lines.get(timeout=max(0.0, deadline - time.time()))
        except queue.Empty:
            break
        if line is None:
            proc.kill()
            raise RuntimeError("oqueue serve exited before listening")
        match = re.search(r"listening on (\S+)", line)
        if match:
            return proc, match.group(1)
    proc.kill()
    raise RuntimeError("oqueue serve never printed its listening address")


def consumer(bootstrap: str) -> Consumer:
    return Consumer(
        {
            "bootstrap.servers": bootstrap,
            "group.id": GROUP,
            "auto.offset.reset": "earliest",
            "enable.auto.commit": False,
            "partition.assignment.strategy": "cooperative-sticky",
            "session.timeout.ms": 6000,
            "max.poll.interval.ms": 20000,
        }
    )


def committed_offset(bootstrap: str) -> int:
    c = consumer(bootstrap)
    try:
        [tp] = c.committed([TopicPartition(TOPIC, 0)], timeout=10)
        return tp.offset
    finally:
        c.close()


def main() -> None:
    proc, addr = start_broker()
    try:
        producer = Producer({"bootstrap.servers": addr, "message.timeout.ms": 10000})
        for i in range(RECORDS):
            producer.produce(TOPIC, f"m{i}".encode())
        producer.flush(10)

        c = consumer(addr)
        c.subscribe([TOPIC])
        seen = 0
        deadline = time.time() + 30
        while time.time() < deadline and seen < RECORDS:
            message = c.poll(0.5)
            if message is not None and not message.error():
                seen += 1
        if seen < RECORDS:
            raise AssertionError(f"consumed {seen} of {RECORDS} before committing")
        c.commit(asynchronous=False)
        c.close()

        before = committed_offset(addr)
        if before != RECORDS:
            raise AssertionError(f"committed offset is {before}, expected {RECORDS}")
        print(f"committed offset {before} before restart")
    finally:
        proc.kill()
        proc.wait(timeout=10)

    proc, addr = start_broker()
    try:
        after = committed_offset(addr)
    finally:
        proc.kill()
        proc.wait(timeout=10)

    # librdkafka reports "no committed offset" as OFFSET_INVALID (-1001).
    if after == RECORDS:
        print(f"committed offset {after} after restart")
        print("OFFSETS SURVIVED")
    else:
        print(f"committed offset after restart is {after}, not {RECORDS}")
        print("OFFSETS LOST")


main()
