"""Microscope (operator, full control) and Viewer (read-only). The instrument is split into parts:
stage, camera, light, laser, cooler — each a small object on the same link."""
from .link import Link, Stream


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

    def move(self, dx=0.0, dy=0.0, dz=0.0):
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

    def __init__(self, host=None, name=None, timeout=120.0):
        """host: "1.2.3.4" / "pc:5160" / None = last used, then LAN discovery (remembered after)."""
        self.link = Link(host, self.ROLE, name, timeout)
        self._stream = None

    # --- shared by both roles
    @property
    def status(self) -> dict:
        return self.link.call("status", idempotent=True)

    @property
    def stream(self) -> Stream:
        """Data connection, opened on first use."""
        if self._stream is None:
            self._stream = Stream(self.link, raw=self.RAW, live=not self.RAW)
        return self._stream

    def frames(self, fps=None):
        """Live frames, forever: `for f in m.frames(): ...`"""
        self.stream.set_live(True, fps)
        return iter(self.stream)

    def latest(self, timeout=5.0):
        """The newest frame (waits for the first one)."""
        s = self.stream
        return s.latest or s.wait(timeout=timeout)

    def on_event(self, fn):
        """fn(event dict) for telemetry / operator / error / closed events. Usable as a decorator."""
        self.link.handlers.append(fn)
        return fn

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
    """Read-only: previews (JPEG), status, events. Any number of these."""

    def fetch(self, seq=0):
        """One full frame from the ring (0 = newest), at most 1/s for viewers."""
        r = self.link.call("frame", idempotent=True, seq=seq, preview=False)
        return self.stream.wait(seq=r["seq"], preview=False)


class Microscope(_Client):
    """Operator: holds the lease (one at a time), full RAW frames, controls everything.
    Live RAW streaming is off until you iterate frames(); snaps always arrive."""
    ROLE = "operator"
    RAW = True

    def __init__(self, host=None, name=None, timeout=120.0):
        super().__init__(host, name, timeout)
        l = self.link
        self.stage, self.camera, self.light, self.laser, self.cooler = Stage(l), Camera(l), Light(l), Laser(l), Cooler(l)

    def snap(self, x=None, y=None, z=None, dx=None, dy=None, dz=None, settle_ms=0, timeout_ms=5000):
        """Move (absolute x/y/z or relative dx/dy/dz, or nowhere), wait until still, return the first frame
        exposed after that. Absolute or no move -> safe to repeat after a reconnect; relative -> not."""
        rel = any(v is not None for v in (dx, dy, dz))
        a = _xyz(x=x, y=y, z=z, dx=dx, dy=dy, dz=dz)
        s = self.stream
        r = self.link.call("snap", idempotent=not rel, settle_ms=settle_ms, timeout_ms=timeout_ms, **a)
        return s.wait(seq=r["seq"], timeout=timeout_ms / 1000 + 5)

    def fetch(self, seq):
        """Re-fetch a frame still in the server ring (e.g. one you missed)."""
        r = self.link.call("frame", idempotent=True, seq=seq)
        return self.stream.wait(seq=r["seq"])

    def stop(self):
        """Stage stops now."""
        self.stage.stop()

    def release(self):
        """Give the lease away without disconnecting."""
        self.link.call("release", idempotent=True)


def _xyz(**kw):
    return {k: float(v) for k, v in kw.items() if v is not None}
