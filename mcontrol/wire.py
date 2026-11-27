"""Wire format shared with the Rust side: [u32 LE len][u8 kind][payload], len counts kind + payload."""
import json
import os
import socket
import struct

K_JSON, K_FRAME = 1, 2
CONTROL_PORT, DATA_PORT, DISCOVERY_PORT = 5160, 5161, 5159
MAX_MSG = 64 << 20


class Wire:
    """One TCP connection that speaks length-prefixed messages."""

    def __init__(self, host: str, port: int, timeout: float = 5.0):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 8 << 20)
        self.sock.settimeout(None)
        self._rf = self.sock.makefile("rb", buffering=1 << 20)

    def send(self, obj: dict) -> None:
        b = json.dumps(obj, separators=(",", ":")).encode()
        self.sock.sendall(struct.pack("<IB", len(b) + 1, K_JSON) + b)

    def recv(self):
        """-> (kind, payload bytes). Raises ConnectionError at EOF."""
        head = self._rf.read(5)
        if len(head) < 5:
            raise ConnectionError("closed")
        n, kind = struct.unpack("<IB", head)
        if not 0 < n <= MAX_MSG:
            raise ConnectionError(f"bad message length {n}")
        p = self._rf.read(n - 1)
        if len(p) < n - 1:
            raise ConnectionError("closed")
        return kind, p

    def recv_json(self) -> dict:
        while True:
            k, p = self.recv()
            if k == K_JSON:
                return json.loads(p)

    def close(self) -> None:
        # shutdown first: it wakes a reader blocked in recv (closing the buffered file first would wait for it)
        for f in (lambda: self.sock.shutdown(socket.SHUT_RDWR), self.sock.close, self._rf.close):
            try:
                f()
            except OSError:
                pass


def discover(wait: float = 0.7, port: int = None) -> list:
    """Servers on the LAN: [(host, control_port, answer dict)], LAN addresses before loopback."""
    import time
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.settimeout(0.1)
    for t in ("255.255.255.255", "127.0.0.1"):
        try:
            s.sendto(b"MCONTROL?", (t, port or int(os.environ.get("MCONTROL_DISCOVERY_PORT", DISCOVERY_PORT))))
        except OSError:
            pass
    out, end = [], time.monotonic() + wait
    while time.monotonic() < end:
        try:
            b, (ip, _) = s.recvfrom(2048)
            v = json.loads(b)
        except (socket.timeout, ValueError):
            continue
        if not any(o[0] == ip or o[2].get("name") == v.get("name") for o in out):
            out.append((ip, int(v.get("control_port", CONTROL_PORT)), v))
    s.close()
    out.sort(key=lambda o: o[0].startswith("127."))
    return out
