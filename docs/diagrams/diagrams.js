// The README diagrams. Each function draws one board; render.py turns each into docs/img/<name>.png.
const D = {};

D.architecture = () => {
  const B = board(1500, 900, 3);
  B.title(40, 50, "Who runs what", "One exe on PC 1, Python on the other PCs");
  B.text(1460, 58, "UDP 5159: clients find PC 1 by themselves", { size: 22, color: "#6b6b6b", anchor: "end" });
  B.frame(40, 110, 760, 750, "PC 1 · microscope PC (Windows 10)");
  const srv = B.box(80, 165, 680, 190, "mc-server.exe",
    "network · operator lease · ring of the last 20 frames\nwatchdog: operator silent 3 s → stage stops, laser off\ncoloured log of everything", { fill: "blue", strong: true, size: 30 });
  const wk = B.box(80, 430, 320, 160, "camera worker", "own process\nMindVision SDK\nraw Bayer, no conversion", { fill: "purple" });
  const gd = B.box(440, 430, 270, 160, "guard", "own process\nheartbeat every 250 ms\nsilent 3 s → acts", { fill: "pink" });
  const cam = B.box(80, 690, 320, 130, "Camera MV-SUA502C", "USB 3 · 2592 × 1944", { fill: "grey" });
  const ctl = B.box(440, 690, 320, 130, "Controller ATR8300", "USB serial (COM)\nknown commands only\nstage · light · laser · TEC", { fill: "grey" });
  B.arrow([240, 690], [240, 590], { label: "USB 3", dx: 42, dy: 0 });
  B.arrow([240, 430], [240, 355], { label: "frames (pipe)", dx: 72, dy: 0 });
  B.arrow([735, 355], [735, 690], { both: true });
  B.arrow([575, 430], [575, 355], { dashed: true, label: "heartbeat", dx: 52, dy: 0, color: "#b4325a" });
  B.arrow([575, 590], [575, 690], { dashed: true, color: "#b4325a" });
  B.text(565, 640, "if the server hangs:\nall off", { size: 19, color: "#b4325a", anchor: "end" });

  B.frame(880, 110, 580, 330, "PC 2 · operator");
  const op = B.box(920, 165, 500, 245, "Python: Microscope()",
    "full RAW frames · snaps\nstage, light, laser, camera\n↓\nyour code: CUDA stitching, scans", { fill: "green", strong: true, size: 30 });
  B.frame(880, 520, 580, 340, "PC 3…N · viewers");
  const vw = B.box(920, 575, 500, 245, "Python: Viewer()",
    "JPEG previews, up to 30 fps\nevery operator action as an event\nread-only, 5 at a time by default", { fill: "yellow", strong: true, size: 30 });
  B.arrow([760, 220], [920, 220], { both: true, label: "control :5160", dy: -18 });
  B.arrow([760, 300], [920, 300], { label: "frames :5161", dy: -18 });
  B.arrow([760, 340], [920, 690], { both: true, bend: "h", mid: 840 });
  B.text(905, 480, "previews + all events", { size: 22, anchor: "start" });
};

D.snap = () => {
  const B = board(1500, 800, 11);
  B.title(40, 50, "snap(x, y, settle_ms): a still, fresh frame", "The frame is the first one whose exposure starts after the stage has stopped and settled");
  const lanes = [["PC 2 · Python", 180], ["mc-server", 300], ["stage", 420], ["camera", 545]];
  for (const [n, y] of lanes) {
    B.text(40, y, n, { size: 26, bold: true, anchor: "start" });
    B.line([230, y], [1460, y], { color: "#bbbbbb", dashed: true });
  }
  B.arrow([270, 180], [270, 300], { label: "snap x=300 settle_ms=30", anchor: "start", at: [288, 240] });
  B.arrow([390, 300], [390, 400], { label: "move in 1 mm steps", anchor: "start", at: [408, 350] });
  B.rect(370, 400, 450, 40, { fill: "orange" }); B.text(595, 420, "moving", { size: 24 });
  B.rect(820, 400, 110, 40, { fill: "yellow" }); B.text(875, 420, "settle", { size: 22 });
  for (const [x, l] of [[820, "stopped"], [930, "t_ready"]]) {
    B.line([x, 150], [x, 590], { color: "#888", dashed: true });
    B.text(x, 610, l, { size: 22, color: "#555" });
  }
  for (let i = 0; i < 14; i++) {
    const x = 250 + i * 80, chosen = x === 970;
    B.rect(x, 525, 64, 40, { fill: chosen ? "green" : x < 930 ? "white" : "grey", width: chosen ? 2.6 : 1.2 });
    if (x < 930) B.cross(x + 32, 545, 10);
  }
  B.text(560, 500, "frames during the move or the settle: never used", { size: 21, color: "#c0392b" });
  B.arrow([1002, 525], [1002, 300], { label: "frame 108\n+ xyz at mid-exposure", anchor: "start", at: [1020, 460], color: "#2f7d32" });
  B.arrow([1060, 300], [1060, 180], { label: "reply {seq: 108}\npixels on the data link", anchor: "start", at: [1078, 240] });
  B.box(230, 670, 1230, 90, "Every frame carries",
    "xyz_um (stage at mid-exposure) · moving · settled_ms · t_exp_start / t_exp_end (server clock) → t_local_ns on PC 2", { fill: "blue", size: 25, bodySize: 22 });
};

D.access = () => {
  const B = board(1600, 930, 5);
  B.title(40, 50, "Who may do what", "Checked for every connection, then for every command");
  B.ellipse(150, 210, 220, 110, "connection\nfrom an address", { fill: "grey" });
  const d1 = B.diamond(440, 210, 240, 140, "address in\n--allow?");
  const d2 = B.diamond(740, 210, 240, 140, "token right?\n(if one is set)");
  const d3 = B.diamond(1040, 210, 240, 140, "wants\ncontrol?");
  const d4 = B.diamond(1040, 430, 240, 140, "address in\n--operators?");
  const d5 = B.diamond(1040, 650, 240, 140, "control\nfree?");
  B.arrow([260, 210], d1.l); B.arrow(d1.r, d2.l, { label: "yes" }); B.arrow(d2.r, d3.l, { label: "yes" });
  B.arrow(d3.b, d4.t, { label: "yes", dx: 26, dy: 0 }); B.arrow(d4.b, d5.t, { label: "yes", dx: 26, dy: 0 });
  const no = (from, to) => {   // label beside a vertical arrow, above a horizontal one
    const vertical = Math.abs(to[0] - from[0]) < 5;
    B.arrow(from, to, { label: "no", color: "#c0392b", dx: vertical ? 22 : 0, dy: vertical ? 0 : -16 });
  };
  const e1 = B.box(340, 360, 200, 90, "E111", "not on the list", { fill: "red", bodySize: 18, size: 26 });
  const e2 = B.box(640, 360, 200, 90, "E112", "no entry", { fill: "red", bodySize: 18, size: 26 });
  no(d1.b, e1.t); no(d2.b, e2.t);
  const v = B.box(1250, 160, 300, 110, "viewer", "watch: previews, events", { fill: "yellow", size: 28 });
  no(d3.r, v.l);
  const e3 = B.box(1300, 385, 250, 90, "E113", "only watch from here", { fill: "red", bodySize: 18, size: 26 });
  no(d4.r, e3.l);
  const e4 = B.box(1300, 605, 250, 90, "E103", "someone else controls", { fill: "red", bodySize: 18, size: 26 });
  no(d5.r, e4.l);
  const op = B.box(580, 600, 240, 100, "operator", "one at a time", { fill: "green", strong: true, size: 30 });
  B.arrow(d5.l, op.r, { label: "yes" });
  B.text(40, 790, "Then every command of the operator:", { size: 26, bold: true, anchor: "start" });
  const steps = [["known Optosky command", "else E305", "blue"], ["inside the soft limits", "else E201", "blue"],
                 ["laser allowed, ≤ max", "else E308 / E309", "blue"], ["controller", "", "grey"]];
  steps.forEach(([t, s, f], i) => {
    const bx = B.box(60 + i * 380, 820, 300, 90, t, s, { fill: f, size: 25, bodySize: 19, bodyColor: "#c0392b" });
    if (i) B.arrow([bx.x - 80, 865], [bx.x, 865]);
  });
};

D.safety = () => {
  const B = board(1500, 830, 9);
  B.title(40, 50, "When something breaks", "Who notices, and what it does");
  const rows = [
    ["PC 2 freezes,\nnetwork drops", "server watchdog\nno ping for 3 s", "stage stops, laser off,\ncontrol released (E105)"],
    ["mc-server hangs\nor crashes", "guard process\nno heartbeat for 3 s", "kills the server, then over COM:\nlaser, motors, light off (E600)"],
    ["camera driver\ncrashes", "camera worker\nis its own process", "restarted by the server;\nthe stage keeps working (E506)"],
    ["link drops during\na command", "Python client", "reconnects; repeats goto and settings,\nnever a relative move (E110)"],
    ["Ctrl+C on PC 1", "mc-server", "stage stops; laser, motors,\nlight off; then exits"],
  ];
  B.text(230, 130, "what happens", { size: 24, color: "#6b6b6b" });
  B.text(690, 130, "who notices", { size: 24, color: "#6b6b6b" });
  B.text(1190, 130, "what it does", { size: 24, color: "#6b6b6b" });
  rows.forEach(([a, b, c], i) => {
    const y = 160 + i * 130;
    const A = B.box(60, y, 340, 105, a, "", { fill: "pink", size: 25 });
    const M = B.box(520, y, 340, 105, b, "", { fill: "blue", size: 25, bold: false });
    const C = B.box(980, y, 440, 105, c, "", { fill: "green", size: 23, bold: false });
    B.arrow(A.r, M.l); B.arrow(M.r, C.l);
  });
};

D.events = () => {
  const B = board(1500, 760, 21);
  B.title(40, 50, "Viewers see everything the operator does", "Every command goes to all clients twice: when it starts, and with its result");
  const op = B.box(40, 230, 330, 170, "PC 2 · operator", "m.snap(x=300)\nm.light.level = 3\nm.laser.mw = 5", { fill: "green", size: 28 });
  const s = B.box(520, 190, 400, 250, "mc-server",
    "1. event: command started\n2. runs it\n3. log line + event: done\n(result, time, who)\nor: failed (error code)", { fill: "blue", strong: true, size: 30 });
  B.arrow([370, 290], [520, 290], { label: "command" });
  B.arrow([520, 350], [370, 350], { label: "reply", dy: 18 });
  const v = B.box(1060, 130, 400, 300, "PC 3 · v.watch()", "", { fill: "yellow", size: 28, top: true });
  const lines = ["12:31:02  pc-2: snap x=300.0  ...", "12:31:02  pc-2: snap x=300.0", "     frame 108 at (300.0, 0.0, 0.0)", "12:31:03  pc-2: light level=3  ok",
                 "12:31:05  pc-2: laser mw=5", "     FAILED E308 Laser not allowed"];
  lines.forEach((l, i) => B.text(1080, 200 + i * 36, l, { size: 21, anchor: "start", color: l.includes("FAILED") ? "#c0392b" : "#2b2b2b" }));
  const w = B.box(1060, 480, 400, 140, "PC 4 · v.on_event(fn)", "your code reacts:\nlog, alarm, dashboard", { fill: "yellow", size: 28 });
  B.arrow([920, 280], [1060, 280], { label: "events" });
  B.arrow([920, 380], [1060, 550], { bend: "h", mid: 990 });
  B.box(40, 660, 1420, 70, "Events:  command (started / done / failed) · client (connected, disconnected) · operator · safe · error · telemetry (1 Hz)",
    "", { fill: "grey", size: 24, bold: false });
};

D.frames = () => {
  const B = board(1500, 670, 17);
  B.title(40, 50, "One camera, many clients", "Each client has its own sender thread: a slow viewer only loses its own frames");
  const c = B.box(40, 285, 220, 100, "camera worker", "~20 fps", { fill: "purple", size: 28 });
  const r = B.box(340, 235, 300, 200, "frame ring", "last 20 frames\nre-fetch by number", { fill: "blue", strong: true, size: 30 });
  for (let i = 0; i < 6; i++) B.rect(372 + i * 40, 380, 30, 30, { fill: i === 5 ? "green" : "white" });
  B.arrow(c.r, r.l, { label: "frames" });
  const o = B.box(760, 115, 340, 160, "operator outbox", "newest wins; packed: lossless,\nJPEG if the link is slow\nsnaps: lossless, never dropped", { fill: "green", size: 27 });
  const v = B.box(760, 400, 340, 130, "viewer outboxes", "JPEG preview, 25 fps\nencoded once, shared by all", { fill: "yellow", size: 27 });
  B.arrow([640, 300], [760, 195], { bend: "h", mid: 700 }); B.arrow([640, 370], [760, 465], { bend: "h", mid: 700 });
  const p2 = B.box(1220, 140, 240, 110, "PC 2", "0.5–2.8 MB / frame", { fill: "green", size: 30 });
  const p3 = B.box(1220, 410, 240, 110, "PC 3…N", "≈ 5–50 kB / frame", { fill: "yellow", size: 30 });
  B.arrow(o.r, p2.l, { label: "thread" }); B.arrow(v.r, p3.l, { label: "thread each" });
  B.text(40, 620, "PC 1 packs each frame once (5–20 ms): 1 GbE carries ≈ 40 lossless frames/s, 100 Mbit ≈ 10–20 with JPEG.",
    { size: 23, anchor: "start", color: "#444" });
};

D.queue = () => {
  const B = board(1500, 820, 29);
  B.title(40, 50, "One queue, in order", "Every command that changes the instrument gets a number and waits its turn");
  const op = B.box(40, 150, 300, 120, "PC 2 · operator", "your script", { fill: "green", size: 27 });
  const as = B.box(40, 320, 300, 120, "PC 2 · assistant", "MStitch", { fill: "green", size: 27 });
  B.frame(420, 120, 640, 400, "mc-server queue");
  const items = [["#41", "goto x=0", "grey"], ["#42–45", "block: goto · sleep · snap · goto", "blue"], ["#46", "light 2", "grey"], ["#47", "snap x=120 (from MStitch, after the block)", "grey"]];
  items.forEach(([n, t, f], i) => B.box(450, 170 + i * 80, 580, 60, `${n}  ${t}`, "", { fill: f, size: 22, bold: false }));
  B.arrow([340, 210], [450, 200]);
  B.arrow([340, 380], [450, 440]);
  const ex = B.box(1140, 230, 320, 150, "runs one at a time", "stage, light, laser, camera\na block without gaps", { fill: "yellow", size: 25 });
  B.arrow([1060, 305], [1140, 305]);
  const st = B.box(40, 520, 300, 100, "stop", "at once, not queued", { fill: "red", size: 27 });
  B.arrow([340, 570], [1220, 380], { bend: "h", mid: 1100, color: "#c0392b" });
  B.text(720, 550, "stage stops · block ends · queued commands dropped (E116)", { size: 21, color: "#c0392b" });
  const h = B.box(420, 660, 640, 110, "history", "last 2000 commands and messages\nn · who · what · result or error · time", { fill: "purple", size: 26 });
  B.arrow([1360, 380], [900, 660], { bend: "v", mid: 610, label: "every result", at: [1250, 595], anchor: "start" });
  const v = B.box(1140, 660, 320, 110, "anyone", "m.history() · watch() · events", { fill: "yellow", size: 25 });
  B.arrow([1060, 715], [1140, 715]);
};
