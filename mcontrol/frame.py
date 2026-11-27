import json
import struct
import time
from dataclasses import dataclass, field

import numpy as np

# Bayer pattern -> (row, col) of R and B in the 2x2 cell
_PAT = {"rg": ((0, 0), (1, 1)), "gr": ((0, 1), (1, 0)), "gb": ((1, 0), (0, 1)), "bg": ((1, 1), (0, 0))}


@dataclass
class Frame:
    """One camera frame. meta comes from the server; data is the pixel payload (raw sensor or JPEG preview)."""
    meta: dict
    data: bytes = field(repr=False)
    t_local_ns: int = 0          # mid-exposure on this PC's clock (time.perf_counter_ns base), 0 if unknown
    _raw: np.ndarray = field(default=None, repr=False)

    @classmethod
    def parse(cls, payload: bytes, offset_ns: int = 0) -> "Frame":
        hl = struct.unpack_from("<I", payload)[0]
        meta = json.loads(payload[4:4 + hl])
        f = cls(meta, payload[4 + hl:])
        t0, t1 = meta.get("t_exp_start", 0), meta.get("t_exp_end", 0)
        if t0:
            f.t_local_ns = (t0 + t1) // 2 - offset_ns
        return f

    # --- metadata, named
    seq = property(lambda s: s.meta["seq"])
    fmt = property(lambda s: s.meta.get("fmt", ""))
    shape = property(lambda s: (s.meta["h"], s.meta["w"]))
    xyz_um = property(lambda s: tuple(s.meta.get("xyz_um", (0, 0, 0))))
    moving = property(lambda s: bool(s.meta.get("moving")))
    settled_ms = property(lambda s: s.meta.get("settled_ms", 0.0))
    exposure_ms = property(lambda s: (s.meta.get("t_exp_end", 0) - s.meta.get("t_exp_start", 0)) / 1e6)

    def __repr__(self):
        h, w = self.shape
        return (f"<Frame {self.seq} {w}x{h} {self.fmt} at {self.xyz_um} um"
                f"{' MOVING' if self.moving else ''}, {self.codec} {len(self.data) / 1e6:.2f} MB>")

    @property
    def is_preview(self) -> bool:
        return self.fmt == "jpeg"

    @property
    def age_ms(self) -> float:
        """How long ago (on our clock) the exposure was, if the clock is synced."""
        return (time.perf_counter_ns() - self.t_local_ns) / 1e6 if self.t_local_ns else float("nan")

    codec = property(lambda s: s.meta.get("codec", "raw") + (f" {s.meta['quality']}" if "quality" in s.meta else ""))

    @property
    def exact(self) -> bool:
        """True when raw() gives the sensor's pixels exactly (raw or lossless); False after JPEG packing."""
        return self.meta.get("codec", "raw") in ("raw", "lossless")

    def raw(self) -> np.ndarray:
        """Sensor array HxW (uint8 or uint16), unpacked on first use (see codec)."""
        if self.is_preview:
            raise ValueError("preview frame has no raw data: use rgb()")
        if self._raw is None:
            self._raw = unpack(self.meta, self.data)
        return self._raw

    def rgb(self, full: bool = False) -> np.ndarray:
        """Colour image HxWx3. Raw Bayer: full=False -> half size (2x2 superpixel, exact, fast);
        full=True -> full size (bilinear). Preview: decoded JPEG (needs pillow)."""
        if self.is_preview:
            import io
            from PIL import Image
            return np.asarray(Image.open(io.BytesIO(self.data)).convert("RGB"))
        a = self.raw()
        if not self.fmt.startswith("bayer_"):
            return np.repeat(a[..., None], 3, axis=2)
        (ry, rx), (by, bx) = _PAT[self.fmt[6:8]]
        gy, gx = (ry, 1 - rx), (1 - ry, rx)
        if not full:
            g = (a[gy[0]::2, gy[1]::2].astype(np.uint32) + a[gx[0]::2, gx[1]::2]) // 2
            return np.dstack([a[ry::2, rx::2], g.astype(a.dtype), a[by::2, bx::2]])
        return _bilinear(a.astype(np.float32), (ry, rx), (by, bx)).astype(a.dtype)

    def save(self, path: str) -> None:
        """.npy keeps raw + nothing else; anything else (png/jpg/tif) saves the colour image; a .json sidecar keeps meta."""
        if path.endswith(".npy"):
            np.save(path, self.raw())
        elif self.is_preview and path.lower().endswith((".jpg", ".jpeg")):
            open(path, "wb").write(self.data)
        else:
            from PIL import Image
            Image.fromarray(self.rgb(full=True)).save(path)
        with open(path.rsplit(".", 1)[0] + ".json", "w") as f:
            json.dump(self.meta, f, indent=1)


def _bilinear(a, r, b):
    h, w = a.shape
    m = {c: np.zeros((h, w), np.float32) for c in "RGB"}
    m["R"][r[0]::2, r[1]::2] = 1
    m["B"][b[0]::2, b[1]::2] = 1
    m["G"] = 1 - m["R"] - m["B"]
    p = np.pad(a, 1, mode="reflect")
    out = []
    for c in "RGB":
        mp = np.pad(m[c], 1, mode="reflect")
        num = sum(p[1 + dy:h + 1 + dy, 1 + dx:w + 1 + dx] * mp[1 + dy:h + 1 + dy, 1 + dx:w + 1 + dx]
                  for dy in (-1, 0, 1) for dx in (-1, 0, 1))
        den = sum(mp[1 + dy:h + 1 + dy, 1 + dx:w + 1 + dx] for dy in (-1, 0, 1) for dx in (-1, 0, 1))
        out.append(np.where(m[c] > 0, a, num / np.maximum(den, 1e-6)))
    return np.clip(np.dstack(out), 0, None)


def unpack(meta: dict, data: bytes) -> np.ndarray:
    """Pixels of a full frame from how the server packed it (meta["codec"]):
    raw: as is; lossless: zstd + same-colour neighbour differences; jpeg: 4 Bayer colour planes as JPEGs."""
    h, w = meta["h"], meta["w"]
    bits = int("".join(c for c in meta.get("fmt", "") if c.isdigit()) or 8)
    dt = np.uint8 if bits <= 8 else np.uint16
    codec = meta.get("codec", "raw")
    if codec == "raw":
        return np.frombuffer(data, dtype=dt).reshape(h, w)
    if codec == "lossless":
        try:
            import zstandard
        except ImportError:
            raise ImportError("lossless frames need the zstandard package: uv sync, or pip install zstandard") from None
        d = np.frombuffer(zstandard.ZstdDecompressor().decompress(data, max_output_size=meta["raw_bytes"]), dtype=dt).reshape(h, w)
        out = np.empty_like(d)
        for c in range(min(2, w)):              # first two columns: difference to two rows up
            for r0 in (0, 1):
                out[r0::2, c] = np.cumsum(d[r0::2, c], dtype=dt)
        for c in range(min(2, w)):              # then along each row, same colour: difference to two to the left
            out[:, c::2] = np.cumsum(np.concatenate([out[:, c:c + 1], d[:, c + 2::2]], axis=1), axis=1, dtype=dt)
        return out
    if codec == "jpeg":
        import io
        import struct
        from PIL import Image
        out = np.empty((h, w), np.uint8)
        pos = 0
        for oy, ox in ((0, 0), (0, 1), (1, 0), (1, 1)):   # same order as the server: (x,y) = (0,0) (1,0) (0,1) (1,1)
            n = struct.unpack_from("<I", data, pos)[0]
            out[oy::2, ox::2] = np.asarray(Image.open(io.BytesIO(data[pos + 4:pos + 4 + n])))
            pos += 4 + n
        return out
    raise ValueError(f"unknown codec {codec}")
