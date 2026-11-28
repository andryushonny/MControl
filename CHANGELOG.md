# Changelog

## 0.6.20
- Fix: the exported DXF was the empty template (layers, images, lines all missing) in the Windows builds. Git on Windows checks the template out with CR LF line ends, so its markers were never found. Now found either way and the drawing is written with plain LF; .gitattributes keeps the template as it is.

## 0.6.19
- Lattice panel, *Fit to all crosses*: every cross of the lattice looked for on the base map (where the lattice puts it, numbered by it) and the lattice fitted through all of them with the signed ones, least squares (specks taken for crosses dropped). Two signed crosses fix the lattice only near them: far away a small error in either grows (0.2 um over one pitch is 2 um ten pitches on). The message says how far the crosses were off the 2-cross lattice and what is left; much left means the map itself is not true (then Stitch again with the crosses pinned, Summary, Correct um/px). Kept while the signed crosses stay as they were; *Signed crosses only* to drop it.

## 0.6.18
- Seams hardly seen (MStitch 0.6.1, nothing to set): frames evened out against each other before blending (brightness and tilted light, per colour), then blended over 40 px in the background. Summary: the largest correction (%).

## 0.6.17
- Crosses pinned in the stitch (MStitch 0.6.0): every scan and *Stitch again* gives MStitch the lattice's crosses around the selection (anchors.csv), at their true places: with the sample's drawing, at the drawing's pitch (the marks' turn and centre kept). Each found in the frames is pinned in the global solve, so drift cannot pile up along the map.
- Summary: crosses pinned, what is left after pinning (px), the drift they took out, and the camera's scale against the drawing (ppm, from crosses two by two in a frame). *Correct um/px of <objective>* applies it to the objective; then scan or Stitch again.

## 0.6.16
- The lens, more of it: radial k3 (r^6) and decentering p1, p2 (Brown-Conrady) besides k1, k2. *Calibrate, Distortion* fits k1, k2 first, then all five, and keeps the five only if they lower what is left by more than 10 % (else they would fit the noise). Settings shows and edits them. On a synthetic lens with all five: what is left < 0.05 px, p1, p2 found to 3e-4.
- *Calibrate, Scale vs focus*: the scale measured from whole frames at z - dz, z, z + dz (dz in Settings, default 5 um): ppm per um of focus. Each scanned frame is then brought to the scale at the scan's start (resampled with the distortion, one pass), so frames taken at another focus do not stretch the map.

## 0.6.15
- The sample's lithography crosses, chosen once per sample (Lattice panel, *Crosses*): *Optical 30 x 30, 200 um* (opt_7x7_with_el_lit_1) or *E-beam 56 x 56, 90 um* (el_lit_mark); cross (X, Y) at its drawing coordinates. Changing it after crosses are signed asks twice. Numbers outside the drawing are shown in red; the pitch the stage sees against the drawing's (ppm).
- With a drawing the lattice is square (one pitch, right angle; 3+ crosses: least squares, mirrored or not).
- Export with a drawing: in the drawing's own coordinates (X right, Y up, cross (X, Y) at its place), with a DRAWING_CROSSES layer to check the overlay on the lithography drawing.
- Export: every square is cut between its four crosses as they really are on the map (each found near its lattice place; a corner not found follows the found ones), so in the drawing each cross lands exactly on its place, whatever is left of scale, skew or drift in the map.
- Summary: scale against the drawing's pitch when the sample has one.

## 0.6.14
- *Summary* (top bar): every stitch of the project and, for the one picked, its numbers against their targets and their history (mean +- 3 sigma of earlier stitches with the same objective and mode): green within the target, yellow within it but out of the usual range, red out of the target. Crosses around the lattice (rms, max), scale (ppm) and skew (mrad) against it, what Align left, the pairs after the solve, overlap likeness, frames to reshoot, objects whole / cut, the brightness step over overlaps, the background left after the flat field, the stage's landing, blurred frames, the z spread, the surface's tilt, curvature and roughness, the optics calibration left. Kept as summary.json next to each map; *Export CSV* writes metrics.csv. Needs MStitch 0.5.2 for its part.

## 0.6.13
- The base selection: the one the lattice's crosses were marked on (shown *base*; *Make base* to choose another). The lattice moves with its map (*Move map*: arrows 1 map px, Shift 10), and *Align* brings every other map onto the base map cross to cross: each lattice cross in both selections found on both maps, this map moved so its crosses lie on the base's (shift and turn; scale too only for another objective). Without a base map in common, onto the lattice as before, now also without scale. A map is no longer stretched to a lattice marked by hand.
- Crosses marked on a map that was moved (by hand or Align) are found where the map is drawn now (before: where it was stitched).
- *Stitch again* in a selection: the last scan's frames stitched once more (a newer MStitch: objects whole, transparent edges), nothing scanned.
- Settings, *Autofocus window*: z never leaves +- this around where the scan started (or where Focus was pressed); 0 = no limit. Default 50 um.
- The black strips at maps' edges come from MStitch < 0.5.1 (black where no frame reaches); 0.5.1 saves those transparent: update MStitch and *Stitch again*.

## 0.6.12
- Settings, *Autofocus while scanning*: on / off, shown plainly; off means every frame at the z the scan began at (before, z still followed the surface measured earlier). The focus button stays for focusing by hand.

## 0.6.11
- Flat field corrects better: the sensor's dark level is measured too (the light off for a moment, 2 frames at the same exposure, the light back as it was) and taken out before dividing, I' = (I - D) / F + D; without it the edges of darker parts of the sample stayed a little dark. 25 frames (5 x 5) instead of 9: less noise. Not used if the exposure changed while dark (automatic exposure) or it was not dark (room light).
- Settings, *Flat field strength* (0.5 - 2, 1 as measured): the gain raised to this power, for what is left.

## 0.6.10
- *Calibrate, Orientation* works on a plain substrate: the frames (here, a quarter field along x, along y) are compared whole (correlation, coarse to fine, the slow shading taken out first), so an edge, a corner or dust is enough; no texture needed. The scale (um/px) is measured as well, from any starting guess (on the simulator: 0.35 um/px turned and mirrored -> 0.2200, straight).
- From a corner uses the same, so the scale is right there too.

## 0.6.9
- From a corner: which edge runs left-right is answered by watching the stage: *Show me* moves it 1 mm along the red edge and back; the substrate went left-right, or away and back. (Telling it from the map alone is still there.)

## 0.6.8
- Controls, the arrows: *left with right* and *up with down* together with *swap* work as named: first the keys are exchanged (the right key does what the left one did), then swapped. Before, with *swap* on, *left with right* changed the up / down keys instead.

## 0.6.7
- *Controls, From a corner of the substrate*: put a corner on the crosshair and say which it is as you see the substrate (top / bottom, left / right). Three frames (here, a little along x, along y) give the camera's orientation against the stage (which of the 8 straight ways the frames agree with; the corner's edges are enough, no texture needed) and where the substrate is from the corner; its two edges are drawn red and blue on the map, you say which runs left-right for you. Then the screen is turned / mirrored to show the substrate as you see it, and the arrows are set the same way: they move the view (default) or the stage itself (a choice there).

## 0.6.6
- *Calibrate, Flat field* (also in Settings): the frame darker to its edges (vignetting, uneven light) measured for the objective on now: 9 frames around the current spot, half a field apart; per Bayer plane the median of the frames (the sample's features fall out), smoothed by a polynomial of degree 4, mean 1. Kept per objective (project folder `flat/`); *Flat field off*.
- With it, live pictures and scanned frames are divided by it on the raw sensor values (before the distortion is taken out); MStitch then gets an even flat field and does not estimate its own.

## 0.6.5
- mc-server's banner: Control / Data / Web show an address the PC really has now (one of *Addresses*), not a stale one of a network it has left.

## 0.6.4
- mc-server's banner lists every address of the PC with its adapter (*Addresses*); a self-assigned one (169.254.x.x, no DHCP on that network) is marked. Without a route out (no gateway) Control / Data / Web show one of these instead of 127.0.0.1.

## 0.6.3
- *Keys* window (top bar): every action and its key in one table — stage arrows, Z (two pairs, Shift x5), Stop, light, laser on / off, focus here, snap, follow, fit, the tools, cancel / done. *Change*, then press the key (with Shift / Ctrl if wanted); *None* takes the key away, *Default* gives back the original, *All back to the defaults*. A key used by two actions is red. Kept for next time; the button names show the current keys.

## 0.6.2
- Camera orientation without tilt: *Orientation* picks one of the 8 straight ways only (0/90/180/270 degrees, mirrored or not) and the scale; buttons *Rotate 90*, *Rotate 180*, *Mirror x*, *Mirror y*, *Straight* (takes a tilt away); the current orientation is shown.
- Controls, the arrows: *Exchange left with right*, *up with down*, *swap* (up moves right, down left, right up, left down) — only which way they move the stage, the picture stays; on top of taught arrows too.

## 0.6.1
- mc-server has a web page for anyone in the lab: `http://<PC 1>:8080` (shown in the banner; `--web-port N`, `--no-web`), inside the exe. The live picture (small MJPEG; each open page takes one viewer place), what the operator's program is doing (state, task, progress bar, time left and when it will be done, or when it finished), the operator's PC and address, project, sample, objective, queue, the stage, light and laser, and the last 10 commands. For computers, laptops and tablets. Read only.
- mc-operator tells the server what it is doing every 2 s (the `work` command).

## 0.6.0
- *Controls* window, to make the program comfortable for whoever sits at the microscope, however it stands:
  1. the picture on the screen: rotate left / right by 90 degrees, mirror left-right / up-down (the view only);
  2. the arrows, taught by watching the stage: it moves a little along X and comes back, you click where you saw it go, the same for Y; then each arrow moves the stage the way it points for you, whatever the screen shows; or each arrow set to +-X / +-Y by hand;
  3. the camera: its orientation found from 3 frames, or turned / mirrored by hand.
- The arrow checkboxes of 0.5.9 are replaced by this.

## 0.5.9
- Settings, *Arrow keys*: swap up/down and left/right, reverse left/right, reverse up/down (both: 180 degrees) — only which way the arrows move the stage; the map stays as it is.

## 0.5.8
- Export takes the ticked selections (with a stitched map) only; the tick shows a selection on the map and sends it to export.
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
