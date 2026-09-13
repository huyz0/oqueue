"""A real client over TLS with SASL/PLAIN, and refused without (`M4.18`).

Usage: tls_sasl.py

⚠️ **The leg `M9.21` was waiting for.** `M9` built TLS termination, the
`SASL/PLAIN` mechanism, topic grants and quotas, and wired none of them into
`bin/oqueue serve` — the only `Dispatcher` in the tree was built with plain
`new`, so a broker from `M9`'s own crate answered every request
unauthenticated over cleartext. Nothing but a real client connecting can show
that the wiring is actually there: a unit test can assert the builder was
called, not that a password on the wire is checked.

⚠️ **Two halves, and the second is the one that matters.** Connecting
successfully proves TLS terminates and the credential is accepted. Being
*refused* without the credential proves the broker is not simply ignoring
authentication — which is exactly what it did before this wiring, and which a
success-only test would have passed against.

⚠️ **Its own broker, with its own certificate.** The certificate has to name
the address the client dials, so this script generates one and starts the
broker itself rather than using the harness's shared cleartext one.

Prints `TLS SASL OK` on success.
"""

import pathlib
import queue
import re
import subprocess
import tempfile
import threading
import time

from confluent_kafka import KafkaException, Producer

TOPIC = "tls-sasl"
PRINCIPAL = "alice"
PASSWORD = "secret"


def self_signed(directory: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    """A certificate naming 127.0.0.1, which is what the client dials."""
    cert, key = directory / "cert.pem", directory / "key.pem"
    subprocess.run(
        [
            "openssl", "req", "-x509", "-newkey", "rsa:2048",
            "-keyout", str(key), "-out", str(cert),
            "-days", "1", "-nodes", "-subj", "/CN=localhost",
            "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
        ],
        check=True,
        capture_output=True,
    )
    return cert, key


def start_broker(env: dict) -> tuple[subprocess.Popen, str]:
    """As `offset_survival.py`: a threaded reader, so the deadline is
    enforced by something a blocking `readline` cannot defeat."""
    proc = subprocess.Popen(
        ["target/debug/oqueue", "serve", "127.0.0.1:0"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
        env={**dict(__import__("os").environ), **env},
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
            line = lines.get(timeout=max(0.0, deadline - time.time()))
        except queue.Empty:
            break
        if line is None:
            proc.kill()
            raise RuntimeError("oqueue serve exited before listening")
        # ⚠️ Echoed, because the broker's own startup lines say whether it
        # read the credential file at all — which is the difference between
        # "the password is wrong" and "there is no password configured", and
        # the two look identical from the client.
        if "oqueue:" in line:
            print(f"broker: {line.rstrip()}")
        match = re.search(r"listening on (\S+)", line)
        if match:
            return proc, match.group(1)
    proc.kill()
    raise RuntimeError("oqueue serve never printed its listening address")


def produce(addr: str, cert: pathlib.Path, *, authenticate: bool) -> None:
    config = {
        "bootstrap.servers": addr,
        "security.protocol": "SASL_SSL" if authenticate else "SSL",
        "ssl.ca.location": str(cert),
        "enable.ssl.certificate.verification": True,
        # ⚠️ **Generous, because this leg does strictly more than its
        # siblings** — a TLS handshake and a SASL exchange before the first
        # metadata request — and it runs after five other legs have warmed
        # the container up. A first version used 8 s/4 s, tighter than
        # `rdkafka_roundtrip.py`'s own 10 s/5 s, and failed once inside the
        # harness while passing standalone: exactly the shape of a timeout
        # that is too tight rather than a broker that is wrong.
        #
        # ⚠️ The *unauthenticated* half keeps a short budget on purpose: it
        # is expected to be refused, an authentication failure is permanent
        # rather than retried, and waiting 20 s to confirm a refusal would
        # add that to every harness run.
        "message.timeout.ms": 12000 if authenticate else 8000,
        "socket.timeout.ms": 6000 if authenticate else 4000,
    }
    if authenticate:
        config |= {
            "sasl.mechanism": "PLAIN",
            "sasl.username": PRINCIPAL,
            "sasl.password": PASSWORD,
        }
    # ⚠️ **A delivery callback, not `flush`'s return.** `flush` returns the
    # number of messages still *queued*, and a permanently failed delivery is
    # dequeued with an error — so a refused produce and a successful one both
    # return 0, and a first version of this leg reported success for both.
    failures: list = []
    producer = Producer(config)

    def on_delivery(err, _msg) -> None:
        if err is not None:
            failures.append(err)

    producer.produce(TOPIC, b"m", on_delivery=on_delivery)
    # ⚠️ **`flush`'s own budget must outlast `message.timeout.ms`, or the
    # callback cannot have fired yet.** A second version of this leg set
    # `message.timeout.ms` to 20 s and flushed for 10, so the authenticated
    # half returned with `failures` still empty for a produce that never
    # landed — it reported success against a broker holding the *wrong*
    # password. Review caught it by changing only that password and watching
    # this print `TLS SASL OK`. The first version was wrong differently (it
    # read `flush`'s return, which counts *queued*, not failed), and the pair
    # is why both signals are checked below.
    remaining = producer.flush(30)
    if failures:
        raise KafkaException(str(failures[0]))
    if remaining:
        raise KafkaException(f"{remaining} message(s) neither delivered nor failed")


def main() -> None:
    with tempfile.TemporaryDirectory() as directory:
        directory = pathlib.Path(directory)
        cert, key = self_signed(directory)
        credentials = directory / "credentials"
        credentials.write_text(f"{PRINCIPAL}:{PASSWORD}\n")
        # ⚠️ **A grant as well as a credential**, because they are separate
        # decisions and the broker checks both: authenticating as `alice`
        # earns nothing by itself, and a first version of this leg without
        # this file was refused `TOPIC_AUTHORIZATION_FAILED` — which is the
        # authorization wiring working, not a fault.
        grants = directory / "grants"
        grants.write_text(f"{PRINCIPAL}:{TOPIC}\n")

        proc, addr = start_broker(
            {
                "OQUEUE_TLS_CERT": str(cert),
                "OQUEUE_TLS_KEY": str(key),
                "OQUEUE_CREDENTIALS": str(credentials),
                "OQUEUE_TOPIC_GRANTS": str(grants),
                "OQUEUE_MAX_IN_FLIGHT": "64",
            }
        )
        try:
            produce(addr, cert, authenticate=True)
            print("authenticated over TLS: produced")

            # ⚠️ The half that proves authentication is not being ignored.
            try:
                produce(addr, cert, authenticate=False)
            except Exception as refusal:  # noqa: BLE001 - any refusal will do
                print(f"unauthenticated: refused ({type(refusal).__name__})")
            else:
                raise AssertionError(
                    "an unauthenticated client produced successfully — the "
                    "credential is configured and is not being checked"
                )
        finally:
            proc.kill()
            proc.wait(timeout=10)

    print("TLS SASL OK")


main()
