# Changelog

## 0.5.8
- *Move map* on a selection: the arrows move its map by hand (1 map pixel, Shift 10), not the stage; Enter or Esc: done. To bring neighbouring selections together before Align.
- Align continues from where a map stands now (a shift by hand included) instead of from the stitched placement.
- Opacity per selection (slider on its card): on the map and in the exported squares (PNG alpha, shown by AutoCAD); saved in the project.

## 0.5.7
- Top bar, *Calibrate* menu: **Orientation** (3 frames: how the camera is turned or mirrored against the stage, and its scale; the distortion kept), **Distortion** (9 frames: k1, k2 with orientation and scale), **White balance** (the live picture made grey).

## 0.5.6
- Z also on the . and , keys (> and < on them); the step is set in Settings, Shift makes it five times larger.
- Settings: *swap x and y* beside the mirrors, so the map can be laid out as the stage stands in front of you (any of the 8 ways together with the mirrors); arrows and everything else follow.
- Camera turned or mirrored against the stage: *Calibrate optics: Measure here* first finds which of the 8 orientations (4 turns, mirrored or not) the frames agree with, then measures scale and distortion; Settings → *Camera orientation*: Turn 90°, Mirror x, Mirror y by hand, and *View like the camera* (the map turned so the live picture stands upright, frame horizontal).
- Dark frames on the preview map no longer get holes: a pure black pixel was taken for "no picture" (and showed the frames below through it); dark pixels are now dark.
- *Clear preview* button: forgets the small pictures of where the camera has been (stitched maps stay).

## 0.5.5
- Z by hand: PageUp / PageDown (step in Settings, 5 um by default); x, y, z of the stage in the status bar.
- *Focus* button: finds focus where the stage stands, as a scan does, and stays at the sharpest z (kept in surface.csv).

## 0.5.4
- Crosses get no numbers of their own: the operator signs each with its X, Y; the lattice and the numbers of all squares appear once 2 crosses are signed.
- The number in a square's top left corner is that corner cross's own number (it was the square's lower index); the same in the DXF and the square files. A lattice whose X runs up or down the screen is drawn as on the screen.
- Dragging the map pans it in every tool (Select and Lattice too), handy on a touchpad; a click still selects.
- MStitch runs without a console window on Windows.

## 0.5.3
- Objective changed on the microscope but not in the program: a notice in the corner of the map (the live picture changed while the stage stood still); when the picture matches another objective's scale, it names it and switches with one click.
- A new selection goes to the top of the list; the list is the drawing priority, top drawn over the rest (map and DXF).

## 0.5.2
- Light and laser: F2 light on/off, Shift+F3 laser on, F3 or Space off; buttons in the top bar, level and power in Settings; LASER ON in red in the status bar; Stop also switches the laser off.
- Keyboard shortcuts work after a click on the map (the map no longer keeps the keyboard).
- Stitch check: frames MStitch marks as fine are no longer reported; the warning is one short line (count and the worst three).
- Progress bars: plain green blocks; stitching shows the time so far against what it took per frame before.

## 0.5.1
- mc-operator, Settings: camera exposure, gain and auto exposure (shown as the camera has them; locked while scanning).
- Exposure and gain are remembered per objective and given to the camera again when that objective is put on or the program reconnects.

## 0.5.0
- `mc-operator.exe` is now the operator's program (Rust, GPU): map of the stage with the live picture and a preview map, selections snapped to the motor step, a scan queue grouped by objective (one stop per objective change) with MStitch stitching beside the next scan, status bar with progress and time left.
- Limits per selection, with the overlap: quick ≤ 1200 frames, quality ≤ 225.
- Lattice of crosses from 2–3 marked crosses (sub-pixel), numbers in the top left of each square, *Level grid* for a sample turned on the stage, *Align* of every map onto the lattice, cutting all selections along one lattice into squares, DXF for AutoCAD (layers in list order, µm).
- Optics per objective calibrated with the stage: scale, camera turn, lens distortion (frames corrected before stitching); white balance.
- Autofocus while scanning: sharpness of every frame; a blurred one is looked for at ±5, ±10, ±20, ±50, ±100 µm, then refined to 2.5 µm; x, y, z and sharpness of every frame kept (`focus.csv`, `surface.csv`), the next frame starts from the surface so far.
- Export takes every selection with a stitched map (no separate checkbox).
- The command line tool is `mc-cli.exe`.
- Simulator: the camera starts where the stage is; `--sim-k1` (lens distortion) and `--sim-tilt` (sample turned) for tests.
- Tests: a server is waited for until it is gone before the next one starts (stable on slow machines).

## 0.4.0
- Scripts read as plain Python: `m.goto()`, `m.move()`, `m.sleep()`, `m.snap()` straight on the microscope, one per line.
- `with m.block():` instead of chains: the commands inside run with nothing from other clients between them; each returns its result at once. A block ends with the `with`, on stop, when its client leaves, or after 30 s idle.

## 0.3.1
- Log: queued commands as `#n Snap ...`, with a capital after the number.

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
