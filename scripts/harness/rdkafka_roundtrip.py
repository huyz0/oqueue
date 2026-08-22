"""librdkafka produce+fetch round trip against a broker (M2's completion leg).

Usage: rdkafka_roundtrip.py <bootstrap host:port>

Produces three payloads, then consumes them back from offset 0 and asserts
order and content. Exits non-zero on any failure; prints ROUND TRIP OK on
success — the string kafka-client-harness.sh greps for.

`enable.idempotence` is False by explicit decision (M2.md's risk list):
idempotent produce needs InitProducerId (key 22), which is M11's, and
discovering that during the first client test is the documented way this
milestone slips. docs/protocol-support.md records the limitation.
"""

import sys
import time

from confluent_kafka import Consumer, KafkaError, KafkaException, Producer, TopicPartition

BOOTSTRAP = sys.argv[1]
TOPIC = "harness"
PAYLOADS = [b"one", b"two", b"three"]


def produce() -> None:
    producer = Producer(
        {
            "bootstrap.servers": BOOTSTRAP,
            "enable.idempotence": False,
            "message.timeout.ms": 10000,
            "socket.timeout.ms": 5000,
        }
    )
    delivered = []

    def on_delivery(err, msg):
        if err is not None:
            raise KafkaException(err)
        delivered.append((msg.partition(), msg.offset()))

    for payload in PAYLOADS:
        producer.produce(TOPIC, payload, on_delivery=on_delivery)
    remaining = producer.flush(10)
    assert remaining == 0, f"{remaining} messages undelivered"
    assert len(delivered) == len(PAYLOADS), delivered
    print("PRODUCED", delivered)


def consume() -> None:
    consumer = Consumer(
        {
            "bootstrap.servers": BOOTSTRAP,
            "group.id": "harness",
            "auto.offset.reset": "earliest",
            "enable.auto.commit": False,
            "enable.partition.eof": True,
        }
    )
    consumer.assign([TopicPartition(TOPIC, 0, 0)])
    got = []
    deadline = time.time() + 15
    while len(got) < len(PAYLOADS) and time.time() < deadline:
        msg = consumer.poll(1.0)
        if msg is None:
            continue
        if msg.error():
            if msg.error().code() == KafkaError._PARTITION_EOF:
                continue
            raise KafkaException(msg.error())
        got.append(msg.value())
    consumer.close()
    assert got == PAYLOADS, got
    print("CONSUMED", got)


produce()
consume()
print("ROUND TRIP OK")
