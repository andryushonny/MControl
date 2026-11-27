"""Control connection to mc-server: commands, events, clock sync, reconnect."""
import collections
import itertools
import json
import os
import socket
import threading
import time

from .errors import McError
from .wire import CONTROL_PORT, DATA_PORT, K_FRAME, Wire, discover

_MEMO = os.environ.get("MCONTROL_CONFIG") or os.path.join(os.path.expanduser("~"), ".mcontrol.json")


def _memo(key, value=None):
    """Tiny settings file so the user never types the server address twice."""
    try:
        d = json.load(open(_MEMO))
    except (OSError, ValueError):
        d = {}
    if value is None:
        return d.get(key)
    if d.get(key) != value:
        d[key] = value
        try:
            json.dump(d, open(_MEMO, "w"))
        except OSError:
            pass
    return value


def _split(h):
    """'pc', 'pc:5170', '10.0.0.5', '[fe80::1]:5160', 'fe80::1' -> (host, port)."""
    if h.startswith("["):
        a, _, p = h[1:].partition("]")
        return a, int(p.lstrip(":") or CONTROL_PORT)
    if h.count(":") == 1:
        a, _, p = h.partition(":")
        return a, int(p)
    return h, CONTROL_PORT


def _candidates(host):
    """Where to look, in order: given > MCONTROL_HOST > last good one > LAN discovery."""
    seen = []
    for h in (host, os.environ.get("MCONTROL_HOST"), _memo("host")):
        if h and h != "auto" and h not in seen:
            seen.append(h)
            yield h
            if h == host:           # an explicit address is a wish, not a hint
                return
    for ip, port, _ in discover():
        yield f"{ip}:{port}"


class Link:
    """Control connection. Thread-safe. Reconnects by itself; repeats a command after a reconnect
    only when repeating it is harmless (idempotent: absolute targets, settings, reads)."""

    def __init__(self, host=None, role="viewer", name=None, timeout=120.0, reconnect_s=10.0, token=None):
        self.role, self.name, self.timeout = role, name or socket.gethostname(), timeout
        self.token = token or os.environ.get("MCONTROL_TOKEN")   # else the one remembered for that server
        self.reconnect_s = reconnect_s         # how long an idempotent command waits for the server to come back
        self.host = host
        self._ids = itertools.count(1)
        self._lock = threading.Lock()          # one (re)connect at a time
        self._pending, self._plock = {}, threading.Lock()
        self.offset_ns, self.rtt_ns = 0, None  # server clock - ours; best round trip
        self._samples = collections.deque(maxlen=32)
        self.handlers = []                     # callables(event dict)
        self.on_reconnect = []                 # callables() after a new session is up
        self.w = None
        self.closed = False
        self._connect()
        threading.Thread(target=self._pinger, daemon=True, name="mc-ping").start()

    # ------------------------------------------------------------ connection
    def _connect(self):
        errors = []
        for h in _candidates(self.host):
            ip, port = _split(h)
            token = self.token or (_memo("tokens") or {}).get(h)
            try:
                w = Wire(ip, port)
                hello = {"cmd": "hello", "id": 0, "role": self.role, "name": self.name, "client": "mcontrol-py"}
                if token:
                    hello["token"] = token
                w.sock.settimeout(5.0)             # something else may sit on that port and never answer
                w.send(hello)
                r = w.recv_json()
                while "event" in r:                # an event can overtake the reply to hello
                    r = w.recv_json()
                w.sock.settimeout(None)
            except (OSError, ValueError, ConnectionError) as e:
                errors.append(f"{h}: {e}")
                continue
            if not r.get("ok"):
                w.close()
                e = McError.from_reply(r.get("error", {}))
                if e.code in ("E103", "E111", "E112", "E113"):   # the server said no: final
                    raise e
                errors.append(f"{h}: {e}")        # not a control port (e.g. a remembered port that moved)
                continue
            self.w, self.addr, self.session, self.server = w, ip, r["session"], r.get("server", {})
            _memo("host", h)
            if token:                          # so it is given once per server, not in every script
                _memo("tokens", {**(_memo("tokens") or {}), h: token})
            threading.Thread(target=self._reader, args=(w,), daemon=True, name="mc-ctl").start()
            return
        raise McError("E110", "no mc-server reachable", "; ".join(errors) or "none found on the LAN")

    def _reconnect(self, broken, wait_s=None):
        """New session in place of `broken`; tries for wait_s seconds (None = until it works)."""
        end = None if wait_s is None else time.monotonic() + wait_s
        if not self._lock.acquire(timeout=-1 if wait_s is None else wait_s):
            raise McError("E110", "server not back yet", f"waited {wait_s} s")
        try:
            if self.closed:
                raise McError("E110", "link closed")
            if self.w is not broken:           # somebody already did it
                return
            delay = 0.2
            while True:
                try:
                    self._connect()
                    break
                except McError as e:
                    if e.code != "E110" or self.closed or (end and time.monotonic() > end):
                        raise
                    time.sleep(delay)
                    delay = min(delay * 2, 2.0)
        finally:
            self._lock.release()
        for f in self.on_reconnect:
            f()

    def _reader(self, w):
        try:
            while True:
                m = w.recv_json()
                if "event" in m:
                    for h in self.handlers:
                        h(m)
                else:
                    with self._plock:
                        slot = self._pending.pop(m.get("id"), None)
                    if slot:
                        slot[1] = m
                        slot[0].set()
        except (OSError, ValueError, ConnectionError):
            pass
        with self._plock:                      # every waiter learns now, not after its timeout
            w.dead = True
            for slot in self._pending.values():
                slot[0].set()
            self._pending.clear()
        for h in self.handlers:
            h({"event": "closed"})

    # ------------------------------------------------------------ commands
    def call(self, cmd, idempotent=False, timeout=None, **args):
        """Send a command, return the reply dict. Raises McError with the server's code."""
        for attempt in (0, 1):
            w = self.w
            slot = [threading.Event(), None]
            i = next(self._ids)
            with self._plock:
                if getattr(w, "dead", False):  # the link died before we asked: do not wait on a corpse
                    slot[0].set()
                else:
                    self._pending[i] = slot
            try:
                w.send({"cmd": cmd, "id": i, **args})
                got = slot[0].wait(timeout or self.timeout)
            except OSError:
                got = True
            with self._plock:
                self._pending.pop(i, None)
            r = slot[1]
            if r is not None:
                if not r.get("ok"):
                    raise McError.from_reply(r.get("error", {}))
                return r
            if not got:
                raise McError("E109", "no reply from the server in time", cmd)
            # link lost mid-command. One-shot actions are never repeated (the heartbeat reconnects in the
            # background); idempotent ones wait for the server and are sent once more.
            if not idempotent or attempt:
                raise McError("E110", "connection lost during the command", f"{cmd}: check the state before repeating it")
            self._reconnect(w, self.reconnect_s)
        raise AssertionError

    def _pinger(self):
        """Heartbeat (keeps the operator lease) and clock sync: least-delayed sample of the last 32."""
        while not self.closed:
            t1 = time.perf_counter_ns()
            try:
                r = self.call("ping", idempotent=True, timeout=2.0, t1=t1, rtt_us=(self.rtt_ns or 0) // 1000)
            except McError as e:
                if e.link:
                    try:
                        self._reconnect(self.w)    # background: keep trying until the server is back
                    except McError:
                        pass
                time.sleep(0.25)
                continue
            t4 = time.perf_counter_ns()
            d = (t4 - t1) - (r["t3"] - r["t2"])
            self._samples.append((max(d, 0), ((r["t2"] - t1) + (r["t3"] - t4)) // 2))
            self.rtt_ns, self.offset_ns = min(self._samples)
            time.sleep(0.25)

    def to_local_ns(self, t_server):
        return t_server - self.offset_ns

    def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            self.w.send({"cmd": "bye", "id": 0})
        except OSError:
            pass
        self.w.close()
