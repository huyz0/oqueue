"""Run the Java AdminClient conformance against the shipped binary.

The driver owns process lifetime and jar pinning so the same test works on
Windows, macOS, and Linux without shell-specific process or path assumptions.
"""

from __future__ import annotations

import hashlib
import html
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import tarfile
import time
import xml.etree.ElementTree as ET
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from queue import Empty, Queue
from threading import Lock
from threading import Thread
from urllib.parse import parse_qs, unquote, urlparse


JARS = {
    "kafka-clients-3.9.1.jar": (
        "https://repo1.maven.org/maven2/org/apache/kafka/kafka-clients/3.9.1/"
        "kafka-clients-3.9.1.jar",
        "7568b998572d256f0b7bc0afdc1b7a2588b8b08415c62ce314c864a6851ae9d9",
    ),
    "slf4j-api-1.7.36.jar": (
        "https://repo1.maven.org/maven2/org/slf4j/slf4j-api/1.7.36/slf4j-api-1.7.36.jar",
        "d3ef575e3e4979678dc01bf1dcce51021493b4d11fb7f1be8ad982877c16a1c0",
    ),
}
CLI_URL = "https://archive.apache.org/dist/kafka/3.9.1/kafka_2.13-3.9.1.tgz"
CLI_SHA512 = "1EA204BA73411737A275429CA976D440F007FF0957B90B19BE41DC5A4BAE52617267769BE9F0B5791714D0B3C4C760605BD426FAEA39EDD90763585523FA2CFE"


def fail(message: str) -> "NoReturn":
    print(f"FAIL admin-client conformance: {message}", file=sys.stderr)
    raise SystemExit(1)


def binary() -> pathlib.Path:
    name = "oqueue.exe" if os.name == "nt" else "oqueue"
    path = pathlib.Path("target") / "debug" / name
    if not path.is_file():
        fail(f"{path} is missing; run cargo build -p oqueue first")
    return path


def jar(directory: pathlib.Path, name: str, url: str, expected: str) -> pathlib.Path:
    path = directory / name
    if not path.exists():
        try:
            from urllib.request import urlopen

            with urlopen(url, timeout=30) as response, path.open("wb") as output:
                shutil.copyfileobj(response, output)
        except Exception as error:  # noqa: BLE001 - report the remedy below
            fail(f"could not fetch pinned {name}: {error}")
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        path.unlink(missing_ok=True)
        fail(f"pinned {name} sha256 mismatch")
    return path


def kafka_cli(directory: pathlib.Path) -> pathlib.Path:
    archive = directory / "kafka_2.13-3.9.1.tgz"
    if not archive.exists():
        from urllib.request import urlopen

        try:
            with urlopen(CLI_URL, timeout=60) as response, archive.open("wb") as output:
                shutil.copyfileobj(response, output)
        except Exception as error:  # noqa: BLE001 - report the remedy below
            fail(f"could not fetch the pinned Kafka CLI: {error}")
    if hashlib.sha512(archive.read_bytes()).hexdigest().upper() != CLI_SHA512:
        archive.unlink(missing_ok=True)
        fail("pinned Kafka CLI sha512 mismatch")
    root = directory / "kafka_2.13-3.9.1"
    if not root.is_dir():
        with tarfile.open(archive, "r:gz") as bundle:
            destination = directory.resolve()
            members = bundle.getmembers()
            for member in members:
                target = (destination / member.name).resolve()
                if target != destination and destination not in target.parents:
                    fail(f"pinned Kafka CLI contains an unsafe archive path: {member.name!r}")
            bundle.extractall(destination, members=members)
    return root / "bin"


class S3State:
    def __init__(self) -> None:
        self.lock = Lock()
        self.objects: dict[str, bytes] = {}


class S3Handler(BaseHTTPRequestHandler):
    state = S3State()

    def log_message(self, _format: str, *_args: object) -> None:
        return

    def object_key(self) -> str:
        path = unquote(urlparse(self.path).path).lstrip("/")
        _, _, key = path.partition("/")
        return key

    def send_bytes(self, status: int, payload: bytes, content_type: str = "application/xml") -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        if payload:
            self.wfile.write(payload)

    def do_HEAD(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        key = self.object_key()
        with self.state.lock:
            payload = self.state.objects.get(key)
        if key and payload is None:
            self.send_bytes(404, b"")
        else:
            self.send_bytes(200, b"")

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        query = parse_qs(urlparse(self.path).query)
        if "list-type" in query:
            prefix = query.get("prefix", [""])[0]
            with self.state.lock:
                keys = sorted(key for key in self.state.objects if key.startswith(prefix))
            contents = "".join(
                f"<Contents><Key>{html.escape(key)}</Key><Size>{len(self.state.objects[key])}</Size>"
                "<ETag>\"test\"</ETag><LastModified>2020-01-01T00:00:00.000Z</LastModified>"
                "<StorageClass>STANDARD</StorageClass></Contents>"
                for key in keys
            )
            body = (
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>"
                "<ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">"
                f"<Name>oqueue</Name><Prefix>{html.escape(prefix)}</Prefix>"
                f"<KeyCount>{len(keys)}</KeyCount><MaxKeys>1000</MaxKeys>"
                "<IsTruncated>false</IsTruncated>"
                f"{contents}</ListBucketResult>"
            ).encode()
            self.send_bytes(200, body)
            return
        key = self.object_key()
        with self.state.lock:
            payload = self.state.objects.get(key)
        if payload is None:
            self.send_bytes(404, b"")
            return
        range_header = self.headers.get("Range")
        if range_header and range_header.startswith("bytes="):
            start, _, end = range_header[6:].partition("-")
            begin = int(start)
            finish = int(end) if end else len(payload) - 1
            payload = payload[begin : finish + 1]
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {begin}-{begin + len(payload) - 1}/*")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
            return
        self.send_bytes(200, payload, "application/octet-stream")

    def do_PUT(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        key = self.object_key()
        with self.state.lock:
            existing = self.state.objects.get(key)
            if self.headers.get("If-None-Match") == "*" and existing is not None:
                self.send_bytes(412, b"")
                return
            length = int(self.headers.get("Content-Length", "0"))
            payload = self.rfile.read(length)
            self.state.objects[key] = payload
        self.send_response(200)
        self.send_header("ETag", f'"{hashlib.md5(payload).hexdigest()}"')
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_DELETE(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        key = self.object_key()
        with self.state.lock:
            self.state.objects.pop(key, None)
        self.send_bytes(204, b"")

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
        if "delete" not in urlparse(self.path).query:
            self.send_bytes(501, b"")
            return
        length = int(self.headers.get("Content-Length", "0"))
        root = ET.fromstring(self.rfile.read(length))
        deleted: list[str] = []
        with self.state.lock:
            for element in root.iter():
                if element.tag.endswith("Key") and element.text:
                    self.state.objects.pop(element.text, None)
                    deleted.append(element.text)
        body = "<DeleteResult>" + "".join(f"<Deleted><Key>{html.escape(key)}</Key></Deleted>" for key in deleted) + "</DeleteResult>"
        self.send_bytes(200, body.encode())


def start_s3() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), S3Handler)
    # ThreadingHTTPServer is intentionally daemonized; the explicit shutdown
    # in main still makes test cleanup deterministic on every OS.
    import threading

    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}"


def start(
    binary_path: pathlib.Path,
    role: str | None = None,
    overrides: dict[str, str] | None = None,
    clear_security: bool = False,
    clear_storage: bool = False,
) -> tuple[subprocess.Popen[str], str, list[str]]:
    environment = dict(os.environ)
    environment.setdefault("OQUEUE_MAX_IN_FLIGHT", "64")
    if clear_security:
        for key in (
            "OQUEUE_TLS_CERT",
            "OQUEUE_TLS_KEY",
            "OQUEUE_CREDENTIALS",
            "OQUEUE_ADMIN_GRANTS",
            "OQUEUE_GROUP_GRANTS",
            "OQUEUE_TOPIC_GRANTS",
        ):
            environment.pop(key, None)
    if clear_storage:
        for key in ("OQUEUE_STORE", "OQUEUE_ADMIN_S3_ENDPOINT", "AWS_ENDPOINT", "AWS_BUCKET"):
            environment.pop(key, None)
    if overrides:
        environment.update(overrides)
    if "OQUEUE_ADMIN_S3_ENDPOINT" in environment:
        environment["OQUEUE_STORE"] = "s3"
        environment["AWS_ENDPOINT"] = environment["OQUEUE_ADMIN_S3_ENDPOINT"]
        environment.setdefault("AWS_ALLOW_HTTP", "true")
        environment.setdefault("AWS_ACCESS_KEY_ID", "test")
        environment.setdefault("AWS_SECRET_ACCESS_KEY", "test")
        environment.setdefault("AWS_REGION", "us-east-1")
        environment.setdefault("AWS_BUCKET", "oqueue-m12-admin")
    if role is not None:
        environment["OQUEUE_ROLE"] = role
    process = subprocess.Popen(
        [str(binary_path), "serve", "127.0.0.1:0"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        bufsize=1,
        env=environment,
    )
    assert process.stdout is not None
    lines: Queue[str | None] = Queue()
    output: list[str] = []

    def pump() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            output.append(line)
            lines.put(line)
        lines.put(None)

    Thread(target=pump, daemon=True).start()
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=max(0.05, deadline - time.monotonic()))
        except Empty:
            break
        if line is not None:
            output.append(line)
            match = re.search(r"listening on (\S+)", line)
            if match:
                return process, match.group(1), output
        elif process.poll() is not None:
            break
    process.kill()
    fail(f"role {role or 'combined'} did not listen; output: {''.join(output[-5:])!r}")


def run_java(java: str, classpath: str, bootstrap: str, mode: str) -> str:
    result = subprocess.run(
        [java, "-cp", classpath, "AdminConformance", bootstrap, mode],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=90,
        check=False,
    )
    output = result.stdout + result.stderr
    if result.returncode != 0:
        fail(f"Java AdminClient {mode} failed:\n{output}")
    return output


def run_cli(cli_bin: pathlib.Path, bootstrap: str, *arguments: str) -> str:
    script_name = "kafka-topics.bat" if os.name == "nt" else "kafka-topics.sh"
    command = [str(cli_bin / script_name), "--bootstrap-server", bootstrap, *arguments]
    if os.name == "nt":
        command = ["cmd", "/c", *command]
    result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
    output = result.stdout + result.stderr
    if result.returncode != 0:
        fail(f"Kafka CLI {' '.join(arguments)} failed:\n{output}")
    return output


def run_config_cli(cli_bin: pathlib.Path, bootstrap: str, topic: str, alter: bool = False) -> str:
    script_name = "kafka-configs.bat" if os.name == "nt" else "kafka-configs.sh"
    command = [
        str(cli_bin / script_name),
        "--bootstrap-server",
        bootstrap,
        "--entity-type",
        "topics",
        "--entity-name",
        topic,
    ]
    command.extend(("--alter", "--add-config", "retention.ms=86400000") if alter else ("--describe",))
    if os.name == "nt":
        command = ["cmd", "/c", *command]
    result = subprocess.run(command, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False)
    output = result.stdout + result.stderr
    if result.returncode != 0:
        fail(f"Kafka config CLI failed:\n{output}")
    return output


def security_files(directory: pathlib.Path) -> dict[str, str]:
    openssl = shutil.which("openssl")
    if openssl is None:
        fail("openssl is required to generate the authenticated AdminClient fixture")
    cert = directory / "cert.pem"
    key = directory / "key.pem"
    subprocess.run(
        [
            openssl,
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-keyout",
            str(key),
            "-out",
            str(cert),
            "-days",
            "1",
            "-nodes",
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1",
        ],
        check=True,
        capture_output=True,
    )
    credentials = directory / "credentials"
    credentials.write_text("alice:alice-secret\nbob:bob-secret\n", encoding="utf-8")
    admin_grants = directory / "admin-grants"
    admin_grants.write_text(
        "\n".join(
            f"alice:{operation}"
            for operation in (
                "create_topics",
                "delete_topics",
                "describe_configs",
                "alter_configs",
                "describe_groups",
                "list_groups",
                "alter_quotas",
            )
        )
        + "\n",
        encoding="utf-8",
    )
    group_grants = directory / "group-grants"
    group_grants.write_text("alice:m12-admin-group\n", encoding="utf-8")
    topic_grants = directory / "topic-grants"
    topic_grants.write_text("alice:m12-admin-persist\n", encoding="utf-8")
    return {
        "OQUEUE_TLS_CERT": str(cert),
        "OQUEUE_TLS_KEY": str(key),
        "OQUEUE_CREDENTIALS": str(credentials),
        "OQUEUE_ADMIN_GRANTS": str(admin_grants),
        "OQUEUE_GROUP_GRANTS": str(group_grants),
        "OQUEUE_TOPIC_GRANTS": str(topic_grants),
        "OQUEUE_MAX_IN_FLIGHT": "64",
        "OQUEUE_ADMIN_CERT": str(cert),
    }


def main() -> None:
    java = shutil.which("java")
    javac = shutil.which("javac")
    if java is None or javac is None:
        fail("a JDK with java and javac is required")
    binary_path = binary()
    harness = pathlib.Path("target") / "harness"
    harness.mkdir(parents=True, exist_ok=True)
    kafka_jar = jar(harness, "kafka-clients-3.9.1.jar", *JARS["kafka-clients-3.9.1.jar"])
    slf4j_jar = jar(harness, "slf4j-api-1.7.36.jar", *JARS["slf4j-api-1.7.36.jar"])
    cli_bin = kafka_cli(harness)
    classpath = os.pathsep.join((str(harness), str(kafka_jar), str(slf4j_jar)))
    compile_result = subprocess.run(
        [javac, "-cp", str(kafka_jar), "-d", str(harness), "scripts/harness/AdminConformance.java"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if compile_result.returncode != 0:
        fail(f"AdminConformance.java did not compile:\n{compile_result.stderr}")

    with tempfile.TemporaryDirectory() as directory_name:
        s3_server, s3_endpoint = start_s3()
        security = security_files(pathlib.Path(directory_name))
        security["OQUEUE_ADMIN_S3_ENDPOINT"] = s3_endpoint
        os.environ.update(security)
        os.environ["OQUEUE_ADMIN_PRINCIPAL"] = "alice"
        os.environ["OQUEUE_ADMIN_PASSWORD"] = "alice-secret"
        processes: list[subprocess.Popen[str]] = []
        log_streams: list[list[str]] = []
        captured: list[str] = []
        try:
            combined, address, logs = start(binary_path, overrides=security)
            processes.append(combined)
            log_streams.append(logs)
            captured.extend(logs)
            captured.append(run_java(java, classpath, address, "conformance"))
            print("ok Java AdminClient lifecycle/config/group/quota conformance")
            captured.append(run_java(java, classpath, address, "persistence-create"))
            print("ok durable topic/config write before restart")

            cli_broker, cli_address, logs = start(binary_path, clear_security=True, clear_storage=True)
            processes.append(cli_broker)
            log_streams.append(logs)
            captured.extend(logs)
            run_cli(
                cli_bin,
                cli_address,
                "--create",
                "--topic",
                "m12-cli-topic",
                "--partitions",
                "1",
                "--replication-factor",
                "1",
            )
            cli_topics = run_cli(cli_bin, cli_address, "--list")
            if "m12-cli-topic" not in cli_topics.splitlines():
                fail("kafka-topics.sh did not list the created topic")
            run_config_cli(cli_bin, cli_address, "m12-cli-topic", alter=True)
            cli_config = run_config_cli(cli_bin, cli_address, "m12-cli-topic")
            if "Dynamic configs for topic m12-cli-topic" not in cli_config:
                fail(f"kafka-configs.sh did not describe the topic: {cli_config!r}")
            captured.extend((cli_topics, cli_config))
            cli_broker.terminate()
            cli_broker.wait(timeout=10)
            print("ok Kafka CLI topic/config conformance")

            os.environ["OQUEUE_ADMIN_PRINCIPAL"] = "bob"
            os.environ["OQUEUE_ADMIN_PASSWORD"] = "bob-secret"
            captured.append(run_java(java, classpath, address, "auth-denied"))
            print("ok authenticated authorization denial")
            os.environ["OQUEUE_ADMIN_PRINCIPAL"] = "alice"
            os.environ["OQUEUE_ADMIN_PASSWORD"] = "alice-secret"

            data_plane, data_plane_address, logs = start(binary_path, "data-plane", security)
            processes.append(data_plane)
            log_streams.append(logs)
            captured.extend(logs)
            captured.append(run_java(java, classpath, data_plane_address, "denied"))
            print("ok AdminClient administrative request denied by data-plane role")

            combined.terminate()
            combined.wait(timeout=10)
            restarted, restarted_address, logs = start(binary_path, overrides=security)
            processes.append(restarted)
            log_streams.append(logs)
            captured.extend(logs)
            captured.append(run_java(java, classpath, restarted_address, "persistence-verify"))
            print("ok AdminClient topic/config restart persistence")
        except subprocess.TimeoutExpired as error:
            fail(f"client leg exceeded its 90 second ceiling: {error}")
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            s3_server.shutdown()
            s3_server.server_close()

        for logs in log_streams:
            captured.extend(logs)

    secret_markers = ("alice-secret", "bob-secret", "test-secret")
    combined_output = "\n".join(captured).lower()
    if any(marker in combined_output for marker in secret_markers):
        leaked = [line for line in combined_output.splitlines() if any(marker in line for marker in secret_markers)]
        fail(f"captured AdminClient output contains a configured secret: {leaked!r}")
    print("ok captured-output secret scan")
    print("ADMIN CLIENT CONFORMANCE OK")


if __name__ == "__main__":
    main()
