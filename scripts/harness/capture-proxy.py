"""TCP proxy that records each Kafka request frame (client->broker) to hex files."""
import socket
import struct
import sys
import threading

backend = sys.argv[1]  # host:port of real broker
outdir = sys.argv[2]
bhost, bport = backend.rsplit(":", 1)

listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", 0))
listener.listen(8)
print(f"proxy on 127.0.0.1:{listener.getsockname()[1]}", flush=True)

seq_lock = threading.Lock()
seq = [0]

API_NAMES = {0: "produce", 1: "fetch", 3: "metadata", 18: "apiversions"}


def recv_exact(sock, n):
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            return None
        buf += chunk
    return buf


def client_to_broker(c, b):
    while True:
        size = recv_exact(c, 4)
        if size is None:
            break
        (n,) = struct.unpack(">i", size)
        body = recv_exact(c, n)
        if body is None:
            break
        api, ver = struct.unpack(">hh", body[:4])
        name = API_NAMES.get(api, f"api{api}")
        with seq_lock:
            i = seq[0]
            seq[0] += 1
        with open(f"{outdir}/{i:03d}-{name}-v{ver}.hex", "w") as f:
            f.write(body.hex() + "\n")
        b.sendall(size + body)
    try:
        b.shutdown(socket.SHUT_WR)
    except OSError:
        pass


def broker_to_client(b, c):
    while True:
        data = b.recv(65536)
        if not data:
            break
        c.sendall(data)
    try:
        c.shutdown(socket.SHUT_WR)
    except OSError:
        pass


def handle(c):
    b = socket.create_connection((bhost, int(bport)))
    t1 = threading.Thread(target=client_to_broker, args=(c, b), daemon=True)
    t2 = threading.Thread(target=broker_to_client, args=(b, c), daemon=True)
    t1.start()
    t2.start()
    t1.join()
    t2.join()


while True:
    conn, _ = listener.accept()
    threading.Thread(target=handle, args=(conn,), daemon=True).start()
