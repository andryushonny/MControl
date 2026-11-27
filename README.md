# MControl

Control an **Optosky ATR8300** microscope (stage, focus, light, laser, TEC) and its **MindVision** camera over the network, from Python.

![Who runs what](docs/img/architecture.png)

* **One exe** on the microscope PC, nothing to install there apart from the MindVision SDK. A colour log shows who is connected and what is sent.
* **Full resolution, fast.** Bayer frames go out without colour conversion; PC 1 only packs them (lossless, or JPEG when the link is slow) so even a weak cable gives 10–20 fps. Each frame carries the stage position at mid-exposure and the time on the shared clock.
* **One operator, many viewers.** The operator holds a lease. Viewers are read-only and get JPEG previews at up to 30 fps (25 by default), encoded once per frame and shared by all of them; 5 viewers at a time by default (`--max-viewers`). Every action of the operator is sent to all clients as an event, so viewers see what is done, by whom, and what came out.
* **Local network by default.** Only local addresses may connect unless you say otherwise; see [Network and security](#network-and-security).
* **Keeps its devices.** While it runs, PC 1 holds the controller and the camera: the controller is checked every 2 s and reopened if the cable was pulled or it was switched off and on (light and motors come back, the laser stays off); a camera that stops sending frames is opened again with the operator's exposure and gain; Windows is kept from sleeping. For USB devices that doze off, switch off *USB selective suspend* in the Windows power plan.
* **Safe.** If the operator goes silent, the stage stops and the laser goes off. A separate guard process does the same over COM if the server itself hangs or dies.
* **Stable client.** The Python client finds the server and remembers it, and reconnects by itself. It repeats a command after a reconnect only when repeating is harmless (`goto`, settings, reads); relative moves are never repeated.

## Quick start

**PC 1: the microscope PC (Windows 10)**
1. Install the MindVision SDK (`MindVision Platform Setup`, <https://www.mindvision.com.cn>).
2. Close the Optosky software: it holds the COM port and the camera.
3. Download `mc-server.exe` from [Releases](../../releases) and run it:
   ```
   mc-server.exe                  # finds the controller on its own
   mc-server.exe --port COM4      # or name the port
   mc-server.exe --sim            # simulator, no hardware
   mc-server.exe --help
   ```
   Ctrl+C stops it. Before exiting it stops the stage and switches off the laser, motors and light.

   ![mc-server log](docs/img/server-log.png)

**PC 2: the operator** (with [uv](https://docs.astral.sh/uv/))
```
git clone https://github.com/andryushonny/MControl && cd MControl
uv sync                                    # .venv with mcontrol + numpy, pillow, jupyter, pytest
uv run jupyter lab examples/basics.ipynb   # the walk-through
uv run python my_script.py                 # your own scripts
```
To use it from another uv project: `uv add "mcontrol @ git+https://github.com/andryushonny/MControl"`.
```python
from mcontrol import Microscope

with Microscope() as m:                 # finds the server on the LAN, remembers it
    m.light.level = 3
    m.camera.exposure_ms = 15
    f = m.snap(x=300, y=0, settle_ms=30)   # move, wait until still, a fresh frame
    print(f.xyz_um, f.moving, f.settled_ms)
    img = f.rgb()                          # numpy HxWx3 (f.raw() = sensor data)
    f.save("tile.png")                     # + tile.json with all metadata
```
The full walk-through is in [`examples/basics.ipynb`](examples/basics.ipynb): stage, light, camera, snap, grid scan, live stream, events, viewers, errors.

## How it works

**A snap** gives a still, fresh frame and says where the stage was:

![snap](docs/img/snap.png)

**Frames** go from one camera to many clients without slowing each other down:

![frames](docs/img/frames.png)

**Frame packing.** PC 1 packs full frames so that a slow link carries more. The client chooses with `Microscope(codec=...)`:

| codec | how | size of a 2592 × 1944 frame* | error | fps on 100 Mbit | fps on 1 GbE |
|---|---|---|---|---|---|
| `raw` | as the sensor gives it | 5.0 MB | none | 2 | 23 |
| `lossless` | same-colour neighbour difference + zstd | 2.8 MB | none | 4 | 40 |
| `jpeg` 92 | each Bayer colour plane as JPEG | 1.0 MB | ≈ 2.5 DN rms | 11 | camera limit |
| `jpeg` 85 | the same, quality 85 | 0.55 MB | ≈ 3 DN rms | 20 | camera limit |
| **`auto`** (default) | snaps and re-fetches **lossless**; live frames lossless, one step down (JPEG 92 → 85 → 75) when the link drops frames, back up when the better step fits | | snaps exact | 10–20 | 40 |

\* a real microscope frame with about 2 DN of sensor noise; the noise decides how small frames get. On PC 1 the packing takes about 5–20 ms per frame (lossless / JPEG planes in parallel); `mc-server codec-bench` measures it on your PC. `Frame.codec` says how a frame came, `Frame.exact` whether its pixels are the sensor's; `Frame.raw()` unpacks.

**Events**: every viewer sees what the operator does, live:

![events](docs/img/events.png)

```python
from mcontrol import Viewer
Viewer().watch()          # prints every operator action, like the server log
```

## Python API

| Object | What |
|---|---|
| `Microscope(codec="auto")` | full frames: `auto`, `lossless`, `jpeg` (with `quality=`), `raw`; see Frame packing |
| `Microscope()` / `Microscope("192.168.1.50")` | operator. Without an address: `MCONTROL_HOST`, then the last server used, then LAN discovery |
| `Viewer(fps=25)` | read-only: previews (`live()`, `latest()`), events, `fetch()` = one full frame per second |
| `Microscope(token="...")` | only when the server asks for a token (or `MCONTROL_TOKEN`); remembered per server |
| `m.stage` | `position`, `goto(x,y,z)`, `move(dx,dy,dz)`, `stop()`, `zero()`, `confirm()`, `motors(on)` |
| `m.camera` | `exposure_ms`, `gain`, `auto_exposure(on)`, `white_balance()`, `state` |
| `m.light.level` | 0..5 |
| `m.laser.mw`, `m.laser.off()` | needs `mc-server --allow-laser` |
| `m.cooler` | `on(bool)`, `temperature` |
| `m.snap(...)` | `x/y/z` absolute or `dx/dy/dz` relative, `settle_ms`; returns a `Frame` |
| `m.live(seconds=, count=, fps=)` | live frames while you loop: `for f in m.live(seconds=5): ...` |
| `m.latest()`, `m.fetch(seq)` | the newest frame; a re-fetch from the server ring |
| `m.on_event(fn)`, `m.watch()` | events: every operator command (started / done / failed, with result), clients, telemetry (1 Hz), errors; `watch()` prints them as lines |
| `Frame` | `raw()`, `rgb(full=False)`, `save()`, `meta`, `xyz_um`, `moving`, `settled_ms`, `t_local_ns`, `age_ms`, `codec`, `exact` |

## Network and security

The emphasis is the local network; reaching the microscope from outside is possible, with care.

| Option on PC 1 | What |
|---|---|
| `--bind 192.168.1.10` | listen only on this address (a dedicated network card, or a VPN address). Default: all |
| `--control-port`, `--data-port` | 5160 and 5161 by default. If a port is busy, the server takes the next free one of the 9 after it and says so; discovery tells clients the real ports |
| `--discovery-port`, `--no-discovery` | UDP 5159 by default. Without discovery clients give the address: `Microscope("192.168.1.10:5160")` |
| `--allow LIST` | who may connect: `local` (default: loopback, 10/8, 172.16/12, 192.168/16, 169.254/16, 100.64/10, IPv6 local), `any`, addresses, ranges `203.0.113.0/24` |
| `--operators LIST` | who may take control (default: same as `--allow`); everybody else can only watch |
| `--max-viewers N`, `--viewer-fps F` | viewers at the same time (default 5) and their preview rate (default 25, at most 30) |
| `--token TEXT` | shared secret (or env `MC_TOKEN`). Required when `--allow` goes beyond local networks: the server refuses to start otherwise (E121) |

Options can live in `mc-server.conf` next to the exe, one per line, so nobody types them twice:
```
bind = 192.168.1.10
operators = 192.168.1.20
allow-laser = false
```

**From outside the lab.** The safest way: a VPN (WireGuard, Tailscale, ZeroTier). The server stays "local only", the remote PC gets a VPN address (Tailscale's 100.64/10 already counts as local), nothing is opened to the Internet. Forwarding ports on the router with `--allow any --token ...` also works, but the traffic is not encrypted: the token and the frames cross the Internet in clear text, so prefer the VPN.

![Who may do what](docs/img/access.png)

**What protects the instrument**
* Only addresses in `--allow` get an answer; only addresses in `--operators` may control; a token if set; one operator at a time.
* The controller only receives the known Optosky commands (E305 for anything else), stage moves stay inside the soft limits, the laser is blocked unless the server is started with `--allow-laser`, and never above the controller's maximum.
* Operator silent for 3 s: stage stops, laser off. Server hung: the guard process does the same over COM.
* Session ids are random, commands are limited to 64 kB, viewers are limited in number (`--max-viewers`, 5) and in full-frame requests.

![When something breaks](docs/img/safety.png)

## Errors

Each error is an `McError` with a `code`; the server log shows the same codes. Generated by `mc-server errors`:

| Code | Where | Message | What to do |
|---|---|---|---|
| E100 | net | Unknown command | Check the command name; client and server versions should match |
| E101 | net | Bad request | A value is missing or out of range: see the detail after the message |
| E102 | net | Read-only: viewers cannot control the microscope | Connect as operator |
| E103 | net | Another operator holds the microscope | Wait until it disconnects or releases control |
| E104 | net | Say hello first | Client bug: the first message must be hello |
| E105 | net | Operator silent too long: stage stopped, laser off | Check the network; the client reconnects by itself |
| E106 | net | Unknown session for the data connection | Open the control connection first; the client does this by itself |
| E107 | net | Frame no longer in the ring | Ask sooner, or start the server with a larger --ring |
| E108 | net | Too many requests | Viewers may fetch one full frame per second |
| E109 | client | No reply from the server in time | Check that the server runs and the network is up |
| E110 | client | Connection to the server lost | The client reconnects by itself; check the state before repeating a relative move |
| E111 | net | Address not allowed | Add the client's address or network to --allow on the server |
| E112 | net | Wrong or missing token | Give the server's token: Microscope(token=...) or MCONTROL_TOKEN |
| E113 | net | Control not allowed from this address | Connect as viewer, or add the address to --operators on the server |
| E114 | net | Too many viewers | Close a viewer, or start the server with a larger --max-viewers |
| E120 | net | Cannot listen on this address or port | Another program uses the port, or the --bind address is not on this PC |
| E121 | net | Open beyond the local network without a token | Give --token, or keep --allow to local networks |
| E201 | stage | Outside the soft limits | Stay inside the stage range (mc-server --help shows the limits) |
| E202 | stage | Limit switch reached | Move back the other way; check the zero |
| E203 | stage | Move stopped | Someone sent stop, or the operator was lost; move again |
| E205 | stage | Z move too large for one command | Split the Z move into smaller steps |
| E300 | controller | Controller not connected | Plug in the USB cable, or give --port COMx |
| E301 | controller | No answer from the controller | Close the Optosky software, replug the USB, power-cycle the controller |
| E302 | controller | Bad reply from the controller | Usually noise on the line: repeat; replug if it stays |
| E303 | controller | Checksum error | Usually noise on the line: repeat; replug if it stays |
| E304 | controller | Reply to another command | Repeat; another program may be using the port |
| E305 | controller | Command not allowed | Only the known Optosky commands are sent to the controller |
| E306 | controller | Controller reports a failure | Check the motors and the cable; power-cycle the controller |
| E307 | controller | Serial port error | Replug the USB; check the port in Device Manager |
| E308 | controller | Laser not allowed (start mc-server with --allow-laser) | Start the server with --allow-laser |
| E309 | controller | Laser power above the maximum | Ask for no more than the maximum the controller reports |
| E500 | camera | Camera not available | Wait a few seconds after start; see the CAM lines in the server log |
| E501 | camera | No camera found | Plug the camera into USB 3; check it in MindVision's demo program |
| E502 | camera | MindVision camera SDK not found: install it from mindvision.com.cn (Downloads -> SDK) | Install the MindVision SDK, or give --sdk DIR |
| E503 | camera | Camera SDK error | Replug the camera; restart the server |
| E504 | camera | Camera used by another program (close the Optosky software) | Close the Optosky software and MindVision's demo |
| E505 | camera | No frame from the camera | Check the cable and the exposure; the worker restarts by itself |
| E506 | camera | Camera worker stopped | It restarts by itself; if it repeats, replug the camera |
| E600 | guard | Server not answering: guard stopped the stage and switched the laser off | Start mc-server again; look at the log for why it hung |

## Protocol (for other languages)
* **Control** on TCP 5160: messages `[u32 LE length][u8 kind=1][JSON]`. First `{"cmd":"hello","role":"operator"|"viewer","name":...,"token":...}`, then `{"cmd":..., "id":n, ...}` → `{"id":n,"ok":true,...}` or `{"ok":false,"error":{"code","msg"}}`. Events come as `{"event":...}`.
* **Data** on TCP 5161: `{"session":..., "want":"raw"|"preview", "codec":"auto"|"lossless"|"jpeg"|"raw", "quality":92}`, then frames `[len][kind=2][u32 header length][header JSON][payload]`. The header's `codec` says how the payload is packed: `lossless` = zstd of the difference to the same-colour pixel two to the left (first two columns: two rows up), wrapping; `jpeg` = 4 × `[u32 length][JPEG]` for the Bayer planes (x, y) = (0,0), (1,0), (0,1), (1,1).
* **Discovery**: UDP broadcast `MCONTROL?` to port 5159; the answer has the name, the real ports and whether a token is needed.
* **Events**: `command` (`state`: started / done / failed, `by`, `cmd`, `args`, `result`, `ms`), `client`, `operator`, `safe`, `error`, `telemetry`.
* **Clock**: `ping {t1}` → `{t1,t2,t3}`. The offset comes from the sample with the smallest round trip among the last 32.

## Build and test
```
cargo build --release                 # target/release/mc-server(.exe), mc-operator, mc-viewer
uv run pytest                         # integration tests on the simulator
python docs/diagrams/render.py        # the pictures in this README (node + Chrome)
```
Windows binaries are built by GitHub Actions on every push; a new version in Cargo.toml on main becomes a release with them (see CHANGELOG.md).
