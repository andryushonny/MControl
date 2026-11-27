# Changelog

## 0.3.0
- One queue on PC 1 for everything that changes the instrument: commands run one at a time in the order they came, each numbered `#n`; `stop` acts at once, ends the running chain and drops what waits (E116); reads are not queued.
- Chains: `m.chain().goto(...).wait(ms).snap(...).run()` runs as one block, nothing between its steps.
- History: the server keeps the last 2000 commands and messages; `m.history()`, `m.queued()`; the log shows `#n`.
- Assistant role (for MStitch): the operator's rights through the same queue; joins from the operator's PC, or with `m.lease_key`.
- Messages between clients: `send`, `on_message`, `request` / `reply`.
- Python: event handlers run on their own thread, so a handler may send commands.

## 0.2.2
- The Python package lives in `mcontrol/` at the top of the repository and the walk-through is `example.ipynb` next to it, so `from mcontrol import Microscope` works in a notebook opened there, also without installing.

## 0.2.1
- PC 1 keeps its devices while the server runs: the controller is checked every 2 s when idle and reopened if it went away (light and motors given back, laser stays off); a camera that stops giving frames is opened again with the operator's exposure and gain; Windows does not go to sleep.

## 0.2.0
- Frame packing on PC 1: `lossless` (exact, ~2:1), `jpeg` (Bayer planes, ~5–10:1), `raw`; `auto` (default) keeps snaps exact and steps live frames down to JPEG when the link is slow: 10–20 fps on 100 Mbit.
- Network settings: `--bind`, port fallback, `--discovery-port` / `--no-discovery`, `mc-server.conf`.
- Access: `--allow` (local networks by default), `--operators`, `--token` (required beyond local networks), at most 5 viewers (`--max-viewers`).
- Viewers: previews at 25 fps (up to 30); every operator command is sent to all clients as an event (started / done / failed); `Viewer().watch()`.
- Python: `live(seconds=...)`, token remembered per server, `Frame.codec` / `Frame.exact`.
- Error table with what to do (`mc-server errors`); log with a level column; diagrams in the README.

## 0.1.0
- First version: `mc-server.exe` (controller, camera worker, guard), Python client, simulator.
