// A tiny whiteboard: hand-drawn boxes (rough.js), straight or smoothly curved arrows, JetBrains Mono text.
const PAL = {
  yellow: "#fff1a8", blue: "#cfe6ff", green: "#d4f3cf", pink: "#ffd6df", purple: "#e6dcff",
  orange: "#ffe0bf", grey: "#ececec", white: "#ffffff", red: "#ffc9c9",
};
const INK = "#2b2b2b", SOFT = "#6b6b6b";
const FONT = "JetBrains Mono", SCALE = 0.7;    // sizes below are given for a handwriting font; mono is wider
const NS = "http://www.w3.org/2000/svg";

function board(w, h, seed = 7) {
  const svg = document.querySelector("svg");
  svg.setAttribute("width", w); svg.setAttribute("height", h);
  svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
  const rc = rough.svg(svg, { options: { seed, roughness: 1.1, bowing: 0.8, stroke: INK, strokeWidth: 1.6 } });
  const add = n => (svg.appendChild(n), n);
  const B = {};
  B.text = (x, y, s, o = {}) => {
    const lines = String(s).split("\n");
    const size = (o.size || 24) * SCALE, lh = size * (o.lh || 1.35);
    const t = document.createElementNS(NS, "text");
    t.setAttribute("x", x); t.setAttribute("y", y - ((lines.length - 1) * lh) / 2);
    t.setAttribute("font-family", FONT); t.setAttribute("font-size", size);
    t.setAttribute("font-weight", o.bold ? 700 : 400);
    t.setAttribute("fill", o.color || INK);
    t.setAttribute("text-anchor", o.anchor || "middle"); t.setAttribute("dominant-baseline", "middle");
    lines.forEach((l, i) => {
      const sp = document.createElementNS(NS, "tspan");
      sp.setAttribute("x", x); if (i) sp.setAttribute("dy", lh); sp.textContent = l; t.appendChild(sp);
    });
    return add(t);
  };
  // box: title on top (bold), body lines under it; returns anchor points
  B.box = (x, y, w, h, title, body = "", o = {}) => {
    add(rc.rectangle(x, y, w, h, { fill: PAL[o.fill || "white"], fillStyle: o.fillStyle || "solid",
      strokeWidth: o.strong ? 2.4 : 1.6, strokeLineDash: o.dashed ? [8, 7] : undefined }));
    const ts = o.size || 26, bs = o.bodySize || 21;
    const tl = String(title).split("\n").length, n = body ? String(body).split("\n").length : 0;
    const th = ts * SCALE * 1.35 * tl, gap = n ? 12 : 0, bh = n ? bs * SCALE * 1.35 * n : 0;
    // title and body as one block, centred in the box (o.top: block at the top, for boxes filled by hand)
    const top = o.top ? y + 18 : y + (h - th - gap - bh) / 2;
    B.text(x + w / 2, top + th / 2, title, { size: ts, bold: o.bold !== false || n > 0 });
    if (n) B.text(x + w / 2, top + th + gap + bh / 2, body, { size: bs, color: o.bodyColor || INK });
    return { x, y, w, h, l: [x, y + h / 2], r: [x + w, y + h / 2], t: [x + w / 2, y], b: [x + w / 2, y + h],
             at: (fx, fy) => [x + w * fx, y + h * fy] };
  };
  B.frame = (x, y, w, h, label, o = {}) => {
    add(rc.rectangle(x, y, w, h, { fill: PAL[o.fill || "grey"], fillStyle: "hachure", hachureGap: 14,
      fillWeight: 0.6, stroke: o.stroke || "#9a9a9a", strokeWidth: 1.4, strokeLineDash: [10, 8] }));
    B.text(x + 16, y + 22, label, { size: o.size || 27, bold: true, anchor: "start", color: o.color || "#444" });
  };
  B.diamond = (cx, cy, w, h, s, o = {}) => {
    add(rc.polygon([[cx, cy - h / 2], [cx + w / 2, cy], [cx, cy + h / 2], [cx - w / 2, cy]],
      { fill: PAL[o.fill || "yellow"], fillStyle: "solid" }));
    B.text(cx, cy, s, { size: o.size || 21 });
    return { l: [cx - w / 2, cy], r: [cx + w / 2, cy], t: [cx, cy - h / 2], b: [cx, cy + h / 2] };
  };
  B.ellipse = (cx, cy, w, h, s, o = {}) => {
    add(rc.ellipse(cx, cy, w, h, { fill: PAL[o.fill || "green"], fillStyle: "solid" }));
    B.text(cx, cy, s, { size: o.size || 23, bold: o.bold });
  };
  // arrow from a to b: a straight line, or a smooth curve through `via`; the head ends exactly at b
  const path = (d, col, w, dashed) => {
    const p = document.createElementNS(NS, "path");
    p.setAttribute("d", d); p.setAttribute("fill", "none"); p.setAttribute("stroke", col);
    p.setAttribute("stroke-width", w); p.setAttribute("stroke-linecap", "round");
    if (dashed) p.setAttribute("stroke-dasharray", "9 7");
    return add(p);
  };
  const head = (tip, from, col) => {
    const ang = Math.atan2(tip[1] - from[1], tip[0] - from[0]), L = 15, W = 0.42;
    const p = document.createElementNS(NS, "path");
    p.setAttribute("d", `M ${tip[0]} ${tip[1]} L ${tip[0] - L * Math.cos(ang - W)} ${tip[1] - L * Math.sin(ang - W)} ` +
                        `L ${tip[0] - L * Math.cos(ang + W)} ${tip[1] - L * Math.sin(ang + W)} Z`);
    p.setAttribute("fill", col); p.setAttribute("stroke", col); p.setAttribute("stroke-linejoin", "round");
    add(p);
  };
  B.arrow = (a, b, o = {}) => {
    const col = o.color || INK, w = o.width || 2;
    // the line stops where the head starts, so the tip is sharp and lands on b
    const ctrl = o.via ? [2 * o.via[0] - (a[0] + b[0]) / 2, 2 * o.via[1] - (a[1] + b[1]) / 2] : null;
    const dirEnd = ctrl || a, dirStart = ctrl || b;
    const back = (p, q, d) => { const L = Math.hypot(p[0] - q[0], p[1] - q[1]) || 1; return [p[0] + (q[0] - p[0]) * d / L, p[1] + (q[1] - p[1]) * d / L]; };
    const e = back(b, dirEnd, 10), s0 = o.both ? back(a, dirStart, 10) : a;
    path(ctrl ? `M ${s0[0]} ${s0[1]} Q ${ctrl[0]} ${ctrl[1]} ${e[0]} ${e[1]}` : `M ${s0[0]} ${s0[1]} L ${e[0]} ${e[1]}`, col, w, o.dashed);
    head(b, dirEnd, col);
    if (o.both) head(a, dirStart, col);
    if (o.label) {
      const m = o.via || [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
      B.text(m[0] + (o.dx || 0), m[1] + (o.dy ?? -16), o.label, { size: o.size || 21, color: o.labelColor || col, anchor: o.anchor });
    }
  };
  B.title = (x, y, s, sub) => {
    B.text(x, y, s, { size: 40, bold: true, anchor: "start" });
    if (sub) B.text(x, y + 34, sub, { size: 23, color: SOFT, anchor: "start" });
  };
  B.cross = (x, y, r = 12) => {
    add(rc.line(x - r, y - r, x + r, y + r, { stroke: "#d33", strokeWidth: 2.4 }));
    add(rc.line(x - r, y + r, x + r, y - r, { stroke: "#d33", strokeWidth: 2.4 }));
  };
  B.line = (a, b, o = {}) => path(`M ${a[0]} ${a[1]} L ${b[0]} ${b[1]}`, o.color || INK, o.width || 1.6, o.dashed);
  B.rect = (x, y, w, h, o = {}) => add(rc.rectangle(x, y, w, h, { fill: PAL[o.fill || "white"], fillStyle: o.fillStyle || "solid",
    stroke: o.stroke || INK, strokeWidth: o.width || 1.4 }));
  return B;
}
