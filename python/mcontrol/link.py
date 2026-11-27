"""Connections to mc-server: Link (control: commands, events, clock sync, reconnect) and Stream (data: frames)."""
import collections
import itertools
import json
import os
import socket
import threading
import time

from .errors import McError
from .frame import Frame
from .wire import CONTROL_PORT, DATA_PORT, K_FRAME, Wire, discover

_MEMO = os.path.join(os.path.expanduser("~"), ".mcontrol.json")


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

    def __init__(self, host=None, role="viewer", name=None, timeout=120.0, reconnect_s=10.0):
        self.role, self.name, self.timeout = role, name or socket.gethostname(), timeout
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
            ip, _, port = h.partition(":")
            try:
                w = Wire(ip, int(port or CONTROL_PORT))
                w.send({"cmd": "hello", "id": 0, "role": self.role, "name": self.name, "client": "mcontrol-py"})
                r = w.recv_json()
            except OSError as e:
                errors.append(f"{h}: {e}")
                continue
            if not r.get("ok"):
                w.close()
                raise McError.from_reply(r.get("error", {}))   # refusal (E103 lease held) is final
            self.w, self.addr, self.session, self.server = w, ip, r["session"], r.get("server", {})
            _memo("host", h)
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


class Stream:
    """Data connection: frames in a background thread. Keeps the latest frame and the last few by seq,
    so a snap/refetch reply and its pixels meet regardless of order. Follows the Link through reconnects."""

    def __init__(self, link, raw, fps=None, live=True, keep=16):
        self.link, self.raw, self.fps, self.live = link, raw, fps, live
        self._cv = threading.Condition()
        self._recent = collections.OrderedDict()
        self._keep = keep
        self.latest = None
        self.w = None
        self._open()
        link.on_reconnect.append(self._open)

    def _open(self):
        if self.w:
            self.w.close()
        port = int(self.link.server.get("data_port", DATA_PORT))
        w = Wire(self.link.addr, port)
        w.send({"session": self.link.session, "want": "raw" if self.raw else "preview", "fps": self.fps or 0})
        r = w.recv_json()
        if not r.get("ok"):
            raise McError.from_reply(r.get("error", {}))
        self.w = w
        threading.Thread(target=self._reader, args=(w,), daemon=True, name="mc-data").start()
        if not self.live:
            self.link.call("stream", idempotent=True, on=False)

    def set_live(self, on, fps=None):
        if (on, fps) != (self.live, self.fps):
            self.live, self.fps = on, fps or self.fps
            args = {"on": on} | ({"fps": self.fps} if self.fps else {})
            self.link.call("stream", idempotent=True, **args)

    def _reader(self, w):
        try:
            while True:
                k, p = w.recv()
                if k != K_FRAME:
                    continue
                f = Frame.parse(p, self.link.offset_ns)
                with self._cv:
                    self.latest = f
                    self._recent[(f.seq, f.is_preview)] = f
                    while len(self._recent) > self._keep:
                        self._recent.popitem(last=False)
                    self._cv.notify_all()
        except (OSError, ConnectionError, ValueError):
            pass

    def wait(self, seq=None, after=None, timeout=10.0, preview=None):
        """Frame with this seq (preview=None: as this stream sends), or the first one newer than `after`.
        A live preview and a full re-fetch of the same seq are different frames."""
        key = (seq, (not self.raw) if preview is None else preview)
        end = time.monotonic() + timeout
        with self._cv:
            while True:
                if seq is not None and key in self._recent:
                    return self._recent[key]
                if seq is None and self.latest and (after is None or self.latest.seq > after):
                    return self.latest
                left = end - time.monotonic()
                if left <= 0 or not self._cv.wait(left):
                    raise McError("E109", "frame did not arrive in time", f"seq {seq}")

    def __iter__(self):
        last = self.latest.seq if self.latest else 0
        while True:
            f = self.wait(after=last, timeout=30.0)
            last = f.seq
            yield f

    def close(self):
        if self.w:
            self.w.close()
