"""Start the shipped binary in every role and inspect its real ApiVersions reply."""

from __future__ import annotations

import os
import queue
import socket
import struct
import subprocess
import sys
import threading
import time
from pathlib import Path


def read_exact(connection: socket.socket, size: int) -> bytes:
    chunks: list[bytes] = []
    received = 0
    while received < size:
        chunk = connection.recv(size - received)
        if not chunk:
            raise RuntimeError("broker closed before the ApiVersions response")
        chunks.append(chunk)
        received += len(chunk)
    return b"".join(chunks)


def request(connection: socket.socket, body: bytes) -> bool:
    try:
        connection.sendall(struct.pack(">i", len(body)) + body)
        frame_size = struct.unpack(">i", read_exact(connection, 4))[0]
        read_exact(connection, frame_size)
        return True
    except (ConnectionError, OSError, RuntimeError):
        return False


def api_versions(connection: socket.socket) -> set[int]:
    body = struct.pack(">hhihB", 18, 3, 1, -1, 0)
    connection.sendall(struct.pack(">i", len(body)) + body)
    frame_size = struct.unpack(">i", read_exact(connection, 4))[0]
    response = read_exact(connection, frame_size)
    if len(response) < 7:
        raise RuntimeError("short ApiVersions response")
    if struct.unpack(">i", response[:4])[0] != 1:
        raise RuntimeError("ApiVersions correlation id changed")
    error_code = struct.unpack(">h", response[4:6])[0]
    if error_code != 0:
        raise RuntimeError(f"ApiVersions returned error {error_code}")
    count = response[6] - 1
    offset = 7
    keys: set[int] = set()
    for _ in range(count):
        if offset + 7 > len(response):
            raise RuntimeError("truncated ApiVersions table")
        keys.add(struct.unpack(">h", response[offset : offset + 2])[0])
        offset += 7
    return keys


def binary_path() -> Path:
    configured = os.environ.get("OQUEUE_BIN")
    candidates = [Path(configured)] if configured else []
    candidates.extend([Path("target/debug/oqueue"), Path("target/debug/oqueue.exe")])
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise RuntimeError("built oqueue binary not found; run the bounded build gate first")


def metadata_request() -> bytes:
    return struct.pack(">hhih", 3, 1, 2, -1) + struct.pack(">i", -1)


def create_topics_request() -> bytes:
    return struct.pack(">hh i B B B i ? B", 19, 5, 3, 0, 0, 1, 1000, False, 0)


def sasl_handshake_request() -> bytes:
    return struct.pack(">hhihh", 17, 0, 3, -1, 5) + b"PLAIN"


def run_role(
    binary: Path,
    role: str,
    expected: set[int],
    owned: bytes,
    rejected: bytes | None,
) -> None:
    environment = os.environ.copy()
    environment["OQUEUE_ROLE"] = role
    process = subprocess.Popen(
        [str(binary), "serve", "127.0.0.1:0"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=environment,
    )
    try:
        deadline = time.monotonic() + 10
        address = None
        lines: queue.Queue[str] = queue.Queue()

        def collect_output() -> None:
            if process.stdout is not None:
                for line in process.stdout:
                    lines.put(line)

        threading.Thread(target=collect_output, daemon=True).start()
        while time.monotonic() < deadline:
            try:
                line = lines.get(timeout=min(0.1, deadline - time.monotonic()))
            except queue.Empty:
                line = ""
            if line.startswith("listening on "):
                address = line.split()[2].strip()
                break
            if process.poll() is not None:
                raise RuntimeError(f"{role} exited before listening")
        if address is None:
            raise RuntimeError(f"{role} did not publish a listening address")
        host, port_text = address.rsplit(":", 1)
        port = int(port_text)
        with socket.create_connection((host, port), timeout=3) as connection:
            keys = api_versions(connection)
        if keys != expected:
            raise RuntimeError(
                f"{role} advertised the wrong responsibility set: {sorted(keys)}"
            )
        with socket.create_connection((host, port), timeout=3) as connection:
            if not request(connection, owned):
                raise RuntimeError(f"{role} rejected its owned request")
        if rejected is not None:
            with socket.create_connection((host, port), timeout=3) as connection:
                if request(connection, rejected):
                    raise RuntimeError(f"{role} accepted a request outside its role")
        print(f"ok role {role}: owned request accepted and contradictory request closed")
    finally:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main() -> int:
    binary = binary_path()
    coordinator = {8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 32, 33, 36, 44, 48, 49}
    data_plane = {1, 2, 3, 17, 18, 36}
    combined = coordinator | data_plane | {0, 22}
    run_role(binary, "coordinator", coordinator, sasl_handshake_request(), metadata_request())
    run_role(binary, "data-plane", data_plane, metadata_request(), create_topics_request())
    run_role(binary, "combined", combined, metadata_request(), None)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError) as error:
        print(f"role smoke failed: {error}", file=sys.stderr)
        raise SystemExit(1)
