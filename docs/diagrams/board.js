// A tiny whiteboard: hand-drawn boxes, notes and arrows (rough.js) with handwritten text (Caveat).
const PAL = {
  yellow: "#fff1a8", blue: "#cfe6ff", green: "#d4f3cf", pink: "#ffd6df", purple: "#e6dcff",
  orange: "#ffe0bf", grey: "#ececec", white: "#ffffff", red: "#ffc9c9",
};
const INK = "#2b2b2b", SOFT = "#6b6b6b";

function board(w, h, seed = 7) {
  const svg = document.querySelector("svg");
  svg.setAttribute("width", w); svg.setAttribute("height", h);
  svg.setAttribute("viewBox", `0 0 ${w} ${h}`);
  const rc = rough.svg(svg, { options: { seed, roughness: 1.1, bowing: 0.8, stroke: INK, strokeWidth: 1.6 } });
  const add = n => (svg.appendChild(n), n);
  const B = {};
  B.text = (x, y, s, o = {}) => {
    const lines = String(s).split("\n");
    const size = o.size || 24, lh = size * (o.lh || 1.12);
    const t = document.createElementNS("http://www.w3.org/2000/svg", "text");
    t.setAttribute("x", x); t.setAttribute("y", y - ((lines.length - 1) * lh) / 2);
    t.setAttribute("font-family", "Caveat"); t.setAttribute("font-size", size);
    t.setAttribute("font-weight", o.bold ? 700 : 500);
    t.setAttribute("fill", o.color || INK);
    t.setAttribute("text-anchor", o.anchor || "middle"); t.setAttribute("dominant-baseline", "middle");
    lines.forEach((l, i) => {
      const sp = document.createElementNS("http://www.w3.org/2000/svg", "tspan");
      sp.setAttribute("x", x); if (i) sp.setAttribute("dy", lh); sp.textContent = l; t.appendChild(sp);
    });
    return add(t);
  };
  // box: title on top (bold), body lines under it; returns anchor points
  B.box = (x, y, w, h, title, body = "", o = {}) => {
    add(rc.rectangle(x, y, w, h, { fill: PAL[o.fill || "white"], fillStyle: o.fillStyle || "solid",
      strokeWidth: o.strong ? 2.4 : 1.6, strokeLineDash: o.dashed ? [8, 7] : undefined }));
    const ts = o.size || 26;
    if (body || o.top) {
      B.text(x + w / 2, y + 26 + (o.pad || 0), title, { size: ts, bold: true });
      const bs = o.bodySize || 21, n = String(body).split("\n").length;   // body right under the title
      B.text(x + w / 2, y + 26 + ts * 0.5 + 22 + ((n - 1) * bs * 1.12) / 2 + (o.pad || 0), body, { size: bs, color: o.bodyColor || INK });
    } else B.text(x + w / 2, y + h / 2, title, { size: ts, bold: o.bold !== false });
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
  // arrow from a to b, optionally through a middle point (curve); label near the middle
  B.arrow = (a, b, o = {}) => {
    const col = o.color || INK, opt = { stroke: col, strokeWidth: o.width || 1.8, strokeLineDash: o.dashed ? [9, 7] : undefined };
    const pts = o.via ? [a, o.via, b] : [a, b];
    add(o.via ? rc.curve(pts, opt) : rc.line(a[0], a[1], b[0], b[1], opt));
    const head = (p, q) => {
      const ang = Math.atan2(p[1] - q[1], p[0] - q[0]), L = 16, s = 0.42;
      add(rc.line(p[0], p[1], p[0] - L * Math.cos(ang - s), p[1] - L * Math.sin(ang - s), opt));
      add(rc.line(p[0], p[1], p[0] - L * Math.cos(ang + s), p[1] - L * Math.sin(ang + s), opt));
    };
    head(b, pts[pts.length - 2]);
    if (o.both) head(a, pts[1]);
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
  B.line = (a, b, o = {}) => add(rc.line(a[0], a[1], b[0], b[1], { stroke: o.color || INK, strokeWidth: o.width || 1.6,
    strokeLineDash: o.dashed ? [8, 7] : undefined }));
  B.rect = (x, y, w, h, o = {}) => add(rc.rectangle(x, y, w, h, { fill: PAL[o.fill || "white"], fillStyle: o.fillStyle || "solid",
    stroke: o.stroke || INK, strokeWidth: o.width || 1.4 }));
  return B;
}
