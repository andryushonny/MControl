"""Data connection: frames from the server.

A reader thread receives frames and keeps the last few, by number. Two ways to wait for one:
  frame(seq)        a given frame (the reply to snap / fetch names it; it may arrive before or after the reply)
  next_frame(after) the first frame newer than `after` (live viewing)
"""
import threading
import time

from .errors import McError
from .frame import Frame
from .wire import DATA_PORT, K_FRAME, Wire


class Stream:
    KEEP = 16                                   # frames kept for frame(seq)

    def __init__(self, link, raw: bool, live: bool, fps=None):
        self.link, self.raw, self.on, self.fps = link, raw, live, fps
        self.latest = None                      # newest frame
        self._kept = {}                         # (seq, is_preview) -> Frame
        self._cv = threading.Condition()
        self.w = None
        self._open()
        link.on_reconnect.append(self._open)    # new session after a reconnect: open again, same settings

    def _open(self):
        if self.w:
            self.w.close()
        w = Wire(self.link.addr, int(self.link.server.get("data_port", DATA_PORT)))
        hello = {"session": self.link.session, "want": "raw" if self.raw else "preview"}
        if self.fps:
            hello["fps"] = self.fps
        w.send(hello)
        r = w.recv_json()
        if not r.get("ok"):
            raise McError.from_reply(r.get("error", {}))
        self.w = w
        threading.Thread(target=self._read, args=(w,), daemon=True, name="mc-data").start()
        if not self.on:
            self.link.call("stream", idempotent=True, on=False)

    def _read(self, w):
        try:
            while True:
                kind, payload = w.recv()
                if kind != K_FRAME:
                    continue
                f = Frame.parse(payload, self.link.offset_ns)
                with self._cv:
                    self.latest = f
                    self._kept[f.seq, f.is_preview] = f
                    if len(self._kept) > self.KEEP:
                        del self._kept[min(self._kept)]
                    self._cv.notify_all()
        except (OSError, ConnectionError, ValueError):
            pass                                # link gone: Link reconnects and calls _open

    def live(self, on: bool, fps=None):
        """Live frames on or off (frames asked for by snap / fetch always arrive)."""
        if (on, fps or self.fps) != (self.on, self.fps):
            self.on, self.fps = on, fps or self.fps
            self.link.call("stream", idempotent=True, on=on, **({"fps": self.fps} if self.fps else {}))

    def _wait(self, found, timeout, what):
        end = time.monotonic() + timeout
        with self._cv:
            while (f := found()) is None:
                left = end - time.monotonic()
                if left <= 0:
                    raise McError("E109", "frame did not arrive in time", what)
                self._cv.wait(left)
            return f

    def frame(self, seq, preview=None, timeout=10.0) -> Frame:
        key = (seq, (not self.raw) if preview is None else preview)
        return self._wait(lambda: self._kept.get(key), timeout, f"frame {seq}")

    def next_frame(self, after=0, timeout=10.0) -> Frame:
        return self._wait(lambda: self.latest if self.latest and self.latest.seq > after else None, timeout, f"after {after}")

    def close(self):
        if self.w:
            self.w.close()
