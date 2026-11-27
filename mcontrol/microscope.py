"""Microscope (operator, full control) and Viewer (read-only). The instrument is split into parts:
stage, camera, light, laser, cooler — each a small object on the same link."""
import threading
import time
from contextlib import contextmanager

from .errors import McError
from .events import describe
from .link import Link
from .stream import Stream


class _Part:
    """A piece of the instrument. `_set` = idempotent command (safe to repeat), `_do` = one-shot action."""

    def __init__(self, link):
        self._link = link

    def _set(self, cmd, **a):
        return self._link.call(cmd, idempotent=True, **a)

    def _do(self, cmd, **a):
        return self._link.call(cmd, **a)

    def _status(self):
        return self._link.call("status", idempotent=True)["server"]


class Stage(_Part):
    """XYZ stage, µm. goto (absolute) is idempotent; move (relative) is not and is never repeated by itself."""

    @property
    def position(self):
        return tuple(self._status()["stage"]["pos_um"])

    def goto(self, x=None, y=None, z=None):
        return tuple(self._set("goto", **_xyz(x=x, y=y, z=z))["pos_um"])

    def move(self, dx=None, dy=None, dz=None):
        return tuple(self._do("move", **_xyz(dx=dx, dy=dy, dz=dz))["pos_um"])

    def stop(self):
        self._set("stop")

    def zero(self):
        """Current position becomes (0, 0, 0)."""
        self._set("zero")

    def confirm(self):
        """Tell the server the stored position is right (after a restart / manual move)."""
        self._set("confirm")

    def motors(self, on: bool):
        self._set("motors", on=on)


class Camera(_Part):
    @property
    def state(self):
        return self._set("camera_state")

    @property
    def exposure_ms(self):
        return self.state.get("exposure_ms")

    @exposure_ms.setter
    def exposure_ms(self, ms):
        self._set("exposure", ms=ms)

    @property
    def gain(self):
        return self.state.get("gain")

    @gain.setter
    def gain(self, g):
        self._set("gain", value=g)

    def auto_exposure(self, on: bool):
        self._set("auto_exposure", on=on)

    def white_balance(self):
        """One-shot white balance on the current view."""
        self._do("white_balance")


class Light(_Part):
    """Illumination 0..5."""

    @property
    def level(self):
        return self._status()["stage"].get("light")

    @level.setter
    def level(self, v: int):
        self._set("light", level=int(v))


class Laser(_Part):
    """Laser power, mW. Needs mc-server --allow-laser."""

    @property
    def mw(self):
        return self._status()["stage"].get("laser_mw")

    @mw.setter
    def mw(self, v: int):
        self._set("laser", mw=int(v))

    def off(self):
        self.mw = 0


class Cooler(_Part):
    """Detector TEC."""

    @property
    def temperature(self):
        """Controller TEC reply (raw bytes as the controller sends them)."""
        return self._set("temperature").get("raw")

    def on(self, enabled: bool = True):
        self._set("cooling", on=enabled)


class _Client:
    ROLE = "viewer"
    RAW = False
    _fps = None                                 # preview rate asked by a Viewer
    _codec, _quality = "auto", None             # how full frames are packed (Microscope)

    def __init__(self, host=None, name=None, token=None, timeout=120.0, hello=None):
        """host: "192.168.1.50", "pc-1:5170", "[fe80::1]"; omitted: MCONTROL_HOST, the last server used,
        then LAN discovery. token: only if the server asks for one (also MCONTROL_TOKEN); remembered per server."""
        self.link = Link(host, self.ROLE, name, timeout, token=token, hello=hello)
        self._stream = None
        self._waiting = {}                      # request n -> [Event, reply message]
        self.link.handlers.append(self._replies)

    # --- shared by every role
    def history(self, limit=100, since=0) -> list:
        """What was done on the microscope, in order: commands (n, who, what, result or error, ms) and messages.
        The server keeps the last 2000. `describe(e)` makes a line of each; commands still waiting are in queued()."""
        return self.link.call("history", idempotent=True, limit=limit, since=since)["history"]

    def queued(self) -> list:
        """Commands waiting their turn on the server."""
        return self.link.call("history", idempotent=True, limit=0)["queued"]

    # --- messages between clients (e.g. MStitch and the operator's script)
    def send(self, to, data, reply_to=None) -> int:
        """A message to "operator", "all", or a client by name. Returns its number n."""
        return self.link.call("send", to=to, data=data, reply_to=reply_to)["n"]

    def on_message(self, fn):
        """fn(message) for every message to us: {"n", "from", "role", "data", "reply_to"}. Usable as a decorator."""
        self.link.handlers.append(lambda ev: ev.get("event") == "message" and fn(ev))
        return fn

    def request(self, to, data, timeout=60.0):
        """Send and wait for the answer (a message with reply_to = our n). Returns the answer's data."""
        n = self.link.call("send", to=to, data=data)["n"]
        slot = self._waiting.setdefault(n, [threading.Event(), None])
        try:
            if not slot[0].wait(timeout):
                raise McError("E109", "no answer to the request", f"to {to}")
            return slot[1]["data"]
        finally:
            self._waiting.pop(n, None)

    def reply(self, message, data) -> int:
        """Answer a message got through on_message."""
        return self.send(message["from"], data, reply_to=message["n"])

    def _replies(self, ev):
        if ev.get("event") == "message" and ev.get("reply_to") is not None:
            slot = self._waiting.setdefault(ev["reply_to"], [threading.Event(), None])
            slot[1] = ev
            slot[0].set()
    @property
    def status(self) -> dict:
        return self.link.call("status", idempotent=True)

    @property
    def stream(self) -> Stream:
        """Data connection, opened on first use."""
        if self._stream is None:
            self._stream = Stream(self.link, raw=self.RAW, live=not self.RAW, fps=self._fps, codec=self._codec, quality=self._quality)
        return self._stream

    def live(self, seconds=None, count=None, fps=None):
        """Live frames while you loop:  `for f in m.live(seconds=5): ...`
        Stops after `seconds` or `count` frames (or when you break); streaming is switched back afterwards."""
        s = self.stream
        was = s.on
        s.live(True, fps)
        end = None if seconds is None else time.monotonic() + seconds
        try:
            seq, n = (s.latest.seq if s.latest else 0), 0
            while (count is None or n < count) and (end is None or time.monotonic() < end):
                f = s.next_frame(after=seq)
                seq, n = f.seq, n + 1
                yield f
        finally:
            s.live(was)

    def fetch(self, seq=0):
        """A full frame from the server's ring by number (0: the newest), e.g. one you missed.
        Viewers: at most one per second."""
        s = self.stream                         # the data link must be open to receive it
        r = self.link.call("frame", idempotent=True, seq=seq, preview=False)
        return s.frame(r["seq"], preview=False)

    def latest(self, timeout=5.0):
        """The newest frame (waits for the first one)."""
        return self.stream.latest or self.stream.next_frame(timeout=timeout)

    def on_event(self, fn):
        """fn(event dict) for every event: what the operator does (command), clients, telemetry (1 Hz), errors.
        Usable as a decorator. describe(event) turns one into a line of text."""
        self.link.handlers.append(fn)
        return fn

    def watch(self, seconds=None, telemetry=False, out=print):
        """Print what happens on the microscope, one line per event, like the server log.
        Blocks for `seconds` (None: until Ctrl+C)."""
        def show(ev):
            if telemetry or ev.get("event") != "telemetry":
                line = describe(ev)
                if line:
                    out(line)
        self.link.handlers.append(show)
        try:
            time.sleep(seconds) if seconds else threading.Event().wait()
        except KeyboardInterrupt:
            pass
        finally:
            self.link.handlers.remove(show)

    def close(self):
        if self._stream:
            self._stream.close()
        self.link.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def __repr__(self):
        return f"<{type(self).__name__} {self.link.server.get('name')} @ {self.link.addr}>"


class Viewer(_Client):
    """Read-only: previews (JPEG), status, events. Up to --max-viewers at once (5 by default).
    fps: preview rate wanted (the server's --viewer-fps by default, 25; at most 30)."""

    def __init__(self, host=None, name=None, token=None, fps=None, timeout=120.0):
        super().__init__(host, name, token, timeout)
        self._fps = fps


class Microscope(_Client):
    """Operator: holds the lease (one at a time), full RAW frames, controls everything.
    Live RAW frames flow only inside live(); snaps always arrive."""
    ROLE = "operator"
    RAW = True

    def __init__(self, host=None, name=None, token=None, codec="auto", quality=None, timeout=120.0, hello=None):
        """codec for full frames: "auto" (default: snaps exact, live frames as good as the link carries),
        "lossless" (always exact, ~2x smaller), "jpeg" (quality 30..100, default 92), "raw"."""
        self._codec, self._quality = codec, quality
        super().__init__(host, name, token, timeout, hello)
        l = self.link
        self.stage, self.camera, self.light, self.laser, self.cooler = Stage(l), Camera(l), Light(l), Laser(l), Cooler(l)

    def snap(self, x=None, y=None, z=None, dx=None, dy=None, dz=None, settle_ms=None, timeout_ms=None):
        """Move (absolute x/y/z or relative dx/dy/dz, or nowhere), wait until still, return the first frame
        exposed after that. Absolute or no move -> safe to repeat after a reconnect; relative -> not."""
        rel = any(v is not None for v in (dx, dy, dz))
        a = _xyz(x=x, y=y, z=z, dx=dx, dy=dy, dz=dz, settle_ms=settle_ms, timeout_ms=timeout_ms)
        s = self.stream
        r = self.link.call("snap", idempotent=not rel, **a)
        return s.frame(r["seq"], timeout=(timeout_ms or 5000) / 1000 + 5)

    @property
    def lease_key(self):
        """Give this to an Assistant on another PC (on this PC it joins without it)."""
        return self.link.lease_key

    # --- the everyday commands, straight on the microscope: m.goto(), m.sleep(), m.snap() ...
    def goto(self, x=None, y=None, z=None):
        """Move to absolute coordinates, um (axes left out stay). Returns the position."""
        return self.stage.goto(x, y, z)

    def move(self, dx=None, dy=None, dz=None):
        """Move by so many um. Returns the position."""
        return self.stage.move(dx, dy, dz)

    def sleep(self, seconds):
        """Pause, in the server's queue: the next command starts that much later."""
        self.link.call("sleep", idempotent=True, ms=seconds * 1000.0)

    @contextmanager
    def block(self):
        """Commands inside run one after another with nothing from anyone else between them
        (e.g. an assistant such as MStitch):

            with m.block():
                m.goto(x=0, y=0)
                m.sleep(0.05)
                f = m.snap()

        Each command still returns its result at once. The block ends when the `with` ends, on stop(),
        if this client leaves, or after 30 s without a command."""
        self.link.call("begin")
        try:
            yield self
        finally:
            try:
                self.link.call("end")
            except McError:
                pass                            # stop() or a lost link ended it already

    def stop(self):
        """Stage stops now; a block ends and waiting commands are dropped (E116)."""
        self.stage.stop()

    def release(self):
        """Give the lease away without disconnecting."""
        self.link.call("release", idempotent=True)


def _xyz(**kw):
    return {k: float(v) for k, v in kw.items() if v is not None}


class Assistant(Microscope):
    """A helper of the operator, e.g. MStitch on the operator's PC: same rights, same queue, while that
    operator holds control. On the operator's PC no key is needed; elsewhere give the operator's m.lease_key."""
    ROLE = "assistant"

    def __init__(self, host=None, name=None, lease_key=None, token=None, codec="auto", quality=None, timeout=120.0):
        super().__init__(host, name, token, codec, quality, timeout, hello={"join": lease_key} if lease_key else None)
