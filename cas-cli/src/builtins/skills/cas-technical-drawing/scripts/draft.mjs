#!/usr/bin/env node
/**
 * draft.mjs — parametric drafting renderer and mechanical drawing checks.
 *
 * Model-first: parts are axis-aligned boxes placed in one world frame
 * (x = width, left→right; y = depth, back→front; z = height, up). Joints are
 * derived from where positioned parts overlap, so the geometry is never hand
 * drawn — every view is a projection of the same solid model.
 *
 *   node draft.mjs render model.json --out dir/ [--sheet letter|a4|tabloid|a3]
 *        [--variant name] [--plain] [--png] [--dxf] [--no-grid]
 *   node draft.mjs check  model.json [--cutlist cutlist.json] [--json]
 *   node draft.mjs check  drawing.svg [--json] [--print-width-mm N] [--min-text-mm 2.5]
 *   node draft.mjs table  model.json [--format md|csv|json]
 *   node draft.mjs dxf    model.json --view front|top|right|iso -o out.dxf
 *
 * Emitted views: third-angle orthographic set (FRONT / TOP / RIGHT) with an
 * isometric inset, a true 30° isometric sheet, hatched section sheets, an
 * exploded sheet with alignment leaders, per-part cards, and one teaching
 * sheet per distinct joint (separated / assembled / dimensioned section).
 * Drafting conventions are enforced in code: line weights (object 0.7 mm,
 * hidden 0.35 dashed, dimension and extension 0.25, centre 0.25 chain),
 * dimensions outside the view in stacked rows with gapped extension lines
 * and filled arrowheads, unidirectional text, a scale bar and a title block.
 *
 * `check` measures the emitted SVG — it does not trust the generator:
 * isometric axis angles and axis-scale equality, drawn part extents against
 * the model, text/stroke and text/text collisions, dimension chains summing
 * to their overall, label values round-tripping, cut-list parity, and the
 * minimum text height at the stated print scale.
 *
 * No dependencies beyond Node ≥ 18. PNG export shells out to rsvg-convert
 * when present.
 */

import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

// ───────────────────────────── constants ─────────────────────────────
const MM_PER_IN = 25.4;
const COS30 = Math.sqrt(3) / 2;
const SIN30 = 0.5;
const EPS = 1e-6;
const SAMPLE = 1e-3; // occupancy sample offset in model units

const SHEETS = {
  letter: { w: 279.4, h: 215.9 },
  a4: { w: 297, h: 210 },
  tabloid: { w: 431.8, h: 279.4 },
  a3: { w: 420, h: 297 },
};
const SCALES_IN = [1, 2, 4, 8, 12, 16, 24, 32, 48, 64];
const SCALES_MM = [1, 2, 5, 10, 20, 50, 100, 200];

const LW = { obj: 0.7, hid: 0.35, dim: 0.25, center: 0.25, cut: 0.7, leader: 0.25, hatch: 0.25, brk: 0.35, border: 0.5 };
const TXT = { dim: 3, label: 3.5, title: 4.5, block: 3, blockTitle: 5, balloon: 3.5 };
const CHAR_W = 0.58; // width/em estimate used for layout and collision tests
const MARGIN = 10;
const TITLE_BLOCK = { w: 125, h: 28 };
const FONT = "'Helvetica Neue', Helvetica, Arial, 'Liberation Sans', sans-serif";

const TONES = {
  light: { top: "#f5f5f2", front: "#dcdcd7", right: "#bdbdb7" },
  mid: { top: "#e6e6e2", front: "#c4c4bf", right: "#a3a39d" },
  dark: { top: "#8a8a85", front: "#63635f", right: "#45453f" },
};
const INK = "#1b1f24";
const DIM_INK = "#1b1f24";
const HID_INK = "#4a5058";
const GRID_MINOR = "#e3e7e4";
const GRID_MAJOR = "#c5ccc7";

// ───────────────────────────── numbers ─────────────────────────────
export function num(v) {
  if (typeof v === "number") return v;
  if (v == null) return NaN;
  const s = String(v).trim().replace(/[″"']/g, "");
  const m = s.match(/^(-?)(\d+)?(?:[-\s]+)?(?:(\d+)\/(\d+))?$/);
  if (m && (m[2] != null || m[3] != null)) {
    const sign = m[1] === "-" ? -1 : 1;
    const whole = m[2] ? Number(m[2]) : 0;
    const frac = m[3] ? Number(m[3]) / Number(m[4]) : 0;
    return sign * (whole + frac);
  }
  const f = Number(s.replace(/mm$/i, ""));
  return Number.isFinite(f) ? f : NaN;
}

export function fmt(value, units) {
  if (units === "mm") {
    const r = Math.round(value * 10) / 10;
    return Number.isInteger(r) ? String(r) : r.toFixed(1);
  }
  const sign = value < 0 ? "-" : "";
  const abs = Math.abs(value);
  let whole = Math.floor(abs);
  let n = Math.round((abs - whole) * 64);
  let d = 64;
  if (n === 64) { whole += 1; n = 0; }
  while (n > 0 && n % 2 === 0) { n /= 2; d /= 2; }
  if (n === 0) return `${sign}${whole}`;
  if (whole === 0) return `${sign}${n}/${d}`;
  return `${sign}${whole}-${n}/${d}`;
}

const nearly = (a, b, tol = 1e-6) => Math.abs(a - b) <= tol;
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const r3 = (v) => Math.round(v * 1000) / 1000;

// ───────────────────────────── boxes ─────────────────────────────
function box(min, max) {
  return { min: [...min], max: [...max] };
}
function boxFrom(at, dims) {
  return box(at, [at[0] + dims[0], at[1] + dims[1], at[2] + dims[2]]);
}
function boxDims(b) {
  return [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
}
function boxVolume(b) {
  const d = boxDims(b);
  return d[0] * d[1] * d[2];
}
function boxIntersect(a, b) {
  const min = [0, 1, 2].map((i) => Math.max(a.min[i], b.min[i]));
  const max = [0, 1, 2].map((i) => Math.min(a.max[i], b.max[i]));
  for (let i = 0; i < 3; i++) if (max[i] - min[i] <= EPS) return null;
  return box(min, max);
}
function boxTranslate(b, v) {
  return box(b.min.map((c, i) => c + v[i]), b.max.map((c, i) => c + v[i]));
}
function boxCenter(b) {
  return [0, 1, 2].map((i) => (b.min[i] + b.max[i]) / 2);
}
function inBox(b, p) {
  return p[0] > b.min[0] - EPS && p[0] < b.max[0] + EPS &&
    p[1] > b.min[1] - EPS && p[1] < b.max[1] + EPS &&
    p[2] > b.min[2] - EPS && p[2] < b.max[2] + EPS;
}
function inBoxStrict(b, p) {
  return p[0] > b.min[0] + EPS && p[0] < b.max[0] - EPS &&
    p[1] > b.min[1] + EPS && p[1] < b.max[1] - EPS &&
    p[2] > b.min[2] + EPS && p[2] < b.max[2] - EPS;
}

// ───────────────────────────── model ─────────────────────────────
const AXIS = { x: 0, y: 1, z: 2 };
const AXIS_NAME = ["x", "y", "z"];

export function loadModel(file) {
  const raw = JSON.parse(fs.readFileSync(file, "utf8"));
  return resolveModel(raw, path.dirname(path.resolve(file)));
}

export function resolveModel(raw, baseDir = ".") {
  const units = raw.units === "mm" ? "mm" : "in";
  const problems = [];
  const parts = [];
  const byId = new Map();
  let item = 0;
  for (const p of raw.parts || []) {
    if (!p.id) { problems.push(`part without id: ${JSON.stringify(p).slice(0, 60)}`); continue; }
    if (byId.has(p.id)) { problems.push(`duplicate part id ${p.id}`); continue; }
    const size = (p.size || []).map(num);
    const at = (p.at || [0, 0, 0]).map(num);
    const dims = (p.dims || []).map(num);
    if (dims.length !== 3 || dims.some((d) => !(d > 0))) {
      problems.push(`part ${p.id}: dims must be three positive numbers (world extents dx dy dz)`);
      continue;
    }
    const part = {
      id: p.id,
      name: p.name || p.id,
      qty: p.qty == null ? 1 : Number(p.qty),
      material: p.material || "",
      tone: p.tone || "light",
      cutlist: p.cutlist || p.name || p.id,
      grain: p.grain || null,
      size: size.length === 3 ? size : [...dims].sort((a, b) => b - a),
      box: boxFrom(at, dims),
      cuts: [],
      cutInfo: [],
      overlaysOnly: false,
      group: p.group || "",
      notes: p.notes || "",
    };
    for (const c of p.cuts || []) {
      const cat = (c.at || []).map(num);
      const cd = (c.dims || []).map(num);
      if (cat.length !== 3 || cd.length !== 3) { problems.push(`part ${p.id}: cut needs at[3] and dims[3]`); continue; }
      const cb = boxIntersect(boxFrom(cat, cd), part.box);
      if (!cb) { problems.push(`part ${p.id}: cut ${c.name || ""} lies outside the part`); continue; }
      part.cuts.push(cb);
      part.cutInfo.push({ box: cb, kind: c.type || "cut", name: c.name || "", joint: null });
    }
    parts.push(part);
    byId.set(part.id, part);
  }
  parts.forEach((p, i) => (p.item = i + 1));
  // item numbers: identical names share an item number
  const itemByName = new Map();
  item = 0;
  for (const p of parts) {
    if (!itemByName.has(p.name)) itemByName.set(p.name, ++item);
    p.item = itemByName.get(p.name);
  }

  const joints = [];
  for (const j of raw.joints || []) {
    const jr = resolveJoint(j, byId, problems);
    if (jr) joints.push(jr);
  }

  const model = {
    project: raw.project || "Untitled",
    title: raw.title || raw.project || "Untitled",
    units,
    sheet: raw.sheet || "letter",
    date: raw.date || new Date().toISOString().slice(0, 10),
    revision: raw.revision || "A",
    author: raw.author || "draft.mjs",
    reference: raw.reference || null,
    notes: raw.notes || [],
    parts,
    joints,
    sections: (raw.sections || []).map((s) => ({
      name: s.name || "A",
      axis: AXIS[s.axis] ?? 1,
      at: num(s.at),
      look: s.look === "+" ? 1 : -1, // +: viewer on the negative side looking toward +; default viewer on the + side looking toward −
      title: s.title || "",
      dims: s.dims || [],
    })),
    explode: raw.explode || {},
    dims: raw.dims || [],
    overlays: raw.overlays || [],
    variants: raw.variants || {},
    features: raw.features || [],
    grid: raw.grid !== false,
    problems,
    baseDir,
  };
  model.bbox = assemblyBox(parts);
  return model;
}

function assemblyBox(parts) {
  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];
  for (const p of parts) {
    for (let i = 0; i < 3; i++) {
      min[i] = Math.min(min[i], p.box.min[i]);
      max[i] = Math.max(max[i], p.box.max[i]);
    }
  }
  return box(min, max);
}

function inferEntryAxis(male, overlap) {
  const cands = [];
  for (let a = 0; a < 3; a++) {
    const beforeEdge = overlap.min[a] - male.box.min[a] > EPS;
    const afterEdge = male.box.max[a] - overlap.max[a] > EPS;
    if (beforeEdge !== afterEdge) cands.push(a);
  }
  return cands.length === 1 ? cands[0] : null;
}

function resolveJoint(j, byId, problems) {
  const type = j.type || "dado";
  const id = j.id || `${type}-${j.female || j.parts?.[0]}-${j.male || j.parts?.[1]}`;
  if (type === "pocket-screw") {
    const from = byId.get(j.from);
    const into = byId.get(j.into);
    if (!from || !into) { problems.push(`joint ${id}: unknown part`); return null; }
    return { id, type, from, into, at: (j.at || boxCenter(boxIntersect(from.box, into.box) || from.box)).map(num), axis: AXIS[j.axis] ?? 0, angle: num(j.angle ?? 15), count: j.count || 1, note: j.note || "" };
  }
  if (type === "dowel") {
    const a = byId.get(j.parts?.[0]);
    const b = byId.get(j.parts?.[1]);
    if (!a || !b) { problems.push(`joint ${id}: unknown part`); return null; }
    const axis = AXIS[j.axis] ?? 0;
    const dia = num(j.diameter ?? 0.375);
    const depth = num(j.depth ?? 1);
    const holes = [];
    for (const at of j.at || []) {
      const c = at.map(num);
      const hmin = c.map((v, i) => (i === axis ? v - depth : v - dia / 2));
      const hmax = c.map((v, i) => (i === axis ? v + depth : v + dia / 2));
      const hb = box(hmin, hmax);
      for (const p of [a, b]) {
        const cb = boxIntersect(hb, p.box);
        if (cb) { p.cuts.push(cb); p.cutInfo.push({ box: cb, kind: "dowel", name: "dowel", joint: id }); }
      }
      holes.push(hb);
    }
    return { id, type, parts: [a, b], axis, diameter: dia, depth, holes, note: j.note || "" };
  }
  const female = byId.get(j.female);
  const male = byId.get(j.male);
  if (!female || !male) { problems.push(`joint ${id}: unknown part ${j.female}/${j.male}`); return null; }
  const overlap = boxIntersect(female.box, male.box);
  if (!overlap) { problems.push(`joint ${id}: ${male.id} does not overlap ${female.id}; position the male ${j.depth ?? "depth"} into the female`); return null; }
  let axis = j.axis != null ? AXIS[j.axis] : inferEntryAxis(male, overlap);
  if (axis == null) { problems.push(`joint ${id}: cannot infer the entry axis; add "axis"`); return null; }
  const od = boxDims(overlap);
  const others = [0, 1, 2].filter((a) => a !== axis);
  const widthAxis = od[others[0]] <= od[others[1]] ? others[0] : others[1];
  const lengthAxis = others.find((a) => a !== widthAxis);
  const depth = od[axis];
  const width = od[widthAxis];
  const sign = boxCenter(male.box)[axis] >= boxCenter(female.box)[axis] ? 1 : -1;
  // assembly (entry) axis: an axis where the cut is open at the female's end and the male does not
  // extend beyond the overlap (a rabbet drops in from the open end); otherwise the depth axis
  let entry = axis, entrySign = sign;
  if (j.entry != null && AXIS[j.entry] != null) {
    entry = AXIS[j.entry];
    entrySign = boxCenter(male.box)[entry] >= boxCenter(female.box)[entry] ? 1 : -1;
  } else {
    for (const a of others) {
      const noStickOut = !(overlap.min[a] - male.box.min[a] > EPS) && !(male.box.max[a] - overlap.max[a] > EPS);
      const openMax = nearly(overlap.max[a], female.box.max[a], 1e-6), openMin = nearly(overlap.min[a], female.box.min[a], 1e-6);
      if (noStickOut && openMax !== openMin) { entry = a; entrySign = openMax ? 1 : -1; break; }
    }
  }
  const joint = {
    id, type, female, male, overlap, axis, widthAxis, lengthAxis, depth, width, sign, entry, entrySign,
    stated: { width: j.width != null ? num(j.width) : null, depth: j.depth != null ? num(j.depth) : null },
    note: j.note || "",
    occurs: j.occurs || "",
    detail: j.detail !== false,
    label: j.label || "",
  };
  if (type === "tenon" || type === "mortise-tenon" || type === "stub-tenon") {
    const tt = num(j.tenon_thickness ?? j.thickness ?? od[widthAxis] / 3);
    const tw = j.tenon_width != null ? num(j.tenon_width) : od[lengthAxis];
    const tmin = [...overlap.min];
    const tmax = [...overlap.max];
    const cw = (overlap.min[widthAxis] + overlap.max[widthAxis]) / 2;
    tmin[widthAxis] = cw - tt / 2; tmax[widthAxis] = cw + tt / 2;
    const cl = (overlap.min[lengthAxis] + overlap.max[lengthAxis]) / 2;
    tmin[lengthAxis] = cl - tw / 2; tmax[lengthAxis] = cl + tw / 2;
    const tenon = box(tmin, tmax);
    joint.tenon = tenon;
    joint.tenonThickness = tt;
    joint.tenonWidth = tw;
    // male shoulders: overlap minus tenon, as up to four slabs
    const slabs = [];
    const s1 = box([...overlap.min], [...overlap.max]); s1.max[widthAxis] = tmin[widthAxis];
    const s2 = box([...overlap.min], [...overlap.max]); s2.min[widthAxis] = tmax[widthAxis];
    for (const s of [s1, s2]) if (boxVolume(s) > EPS) slabs.push(s);
    if (tw < od[lengthAxis] - EPS) {
      const s3 = box([...overlap.min], [...overlap.max]); s3.max[lengthAxis] = tmin[lengthAxis];
      const s4 = box([...overlap.min], [...overlap.max]); s4.min[lengthAxis] = tmax[lengthAxis];
      for (const s of [s3, s4]) if (boxVolume(s) > EPS) slabs.push(s);
    }
    for (const s of slabs) { male.cuts.push(s); male.cutInfo.push({ box: s, kind: "shoulder", name: "shoulder", joint: id }); }
    female.cuts.push(tenon);
    female.cutInfo.push({ box: tenon, kind: "mortise", name: j.name || "mortise", joint: id });
  } else {
    female.cuts.push(overlap);
    female.cutInfo.push({ box: overlap, kind: type, name: j.name || type, joint: id });
  }
  return joint;
}

// Apply a variant (omit parts / move parts) and return a shallow copy of the parts list.
function variantParts(model, variant) {
  if (!variant) return model.parts;
  const v = model.variants[variant];
  if (!v) throw new Error(`unknown variant ${variant}; defined: ${Object.keys(model.variants).join(", ") || "(none)"}`);
  const omit = new Set(v.omit || []);
  return model.parts.filter((p) => !omit.has(p.id)).map((p) => {
    const mv = v.move?.[p.id];
    if (!mv) return p;
    const d = mv.map(num);
    return { ...p, box: boxTranslate(p.box, d), cuts: p.cuts.map((c) => boxTranslate(c, d)) };
  });
}

// ───────────────────────────── solids ─────────────────────────────
function occ(part, p) {
  if (!inBox(part.box, p)) return false;
  for (const c of part.cuts) if (inBoxStrict(c, p) || inBox(c, p) && insideCutTolerant(c, p)) return false;
  return true;
}
function insideCutTolerant(c, p) {
  // a sample point sits SAMPLE inside a cell; treat cut boundaries as closed toward the cut interior
  return p[0] > c.min[0] - EPS && p[0] < c.max[0] + EPS && p[1] > c.min[1] - EPS && p[1] < c.max[1] + EPS && p[2] > c.min[2] - EPS && p[2] < c.max[2] + EPS;
}

function coordSets(parts) {
  const sets = [new Set(), new Set(), new Set()];
  for (const p of parts) {
    for (const b of [p.box, ...p.cuts]) for (let i = 0; i < 3; i++) { sets[i].add(r6(b.min[i])); sets[i].add(r6(b.max[i])); }
  }
  return sets.map((s) => [...s].sort((a, b) => a - b));
}
const r6 = (v) => Math.round(v * 1e6) / 1e6;

/** Edges of one part: segments where the 4-quadrant occupancy pattern is a real edge. */
function partEdges(part) {
  const sets = coordSets([part]);
  const edges = [];
  for (let a = 0; a < 3; a++) {
    const b = (a + 1) % 3, c = (a + 2) % 3;
    for (const vb of sets[b]) for (const vc of sets[c]) {
      let run = null;
      for (let i = 0; i + 1 < sets[a].length; i++) {
        const m = (sets[a][i] + sets[a][i + 1]) / 2;
        let mask = 0;
        const quads = [[+1, +1], [-1, +1], [-1, -1], [+1, -1]];
        quads.forEach(([sb, sc], qi) => {
          const p = [0, 0, 0]; p[a] = m; p[b] = vb + sb * SAMPLE; p[c] = vc + sc * SAMPLE;
          if (occ(part, p)) mask |= 1 << qi;
        });
        const flat = mask === 0 || mask === 15 || mask === 3 || mask === 6 || mask === 12 || mask === 9;
        const kind = flat ? null : ([1, 2, 4, 8].includes(mask) ? "convex" : "concave");
        if (kind && run && run.kind === kind) { run.t1 = sets[a][i + 1]; }
        else {
          if (run) edges.push(finishEdge(run, a, b, c, vb, vc, part));
          run = kind ? { kind, t0: sets[a][i], t1: sets[a][i + 1] } : null;
        }
      }
      if (run) edges.push(finishEdge(run, a, b, c, vb, vc, part));
    }
  }
  return edges;
}
function finishEdge(run, a, b, c, vb, vc, part) {
  const p0 = [0, 0, 0], p1 = [0, 0, 0];
  p0[a] = run.t0; p1[a] = run.t1; p0[b] = p1[b] = vb; p0[c] = p1[c] = vc;
  return { axis: a, p0, p1, part, kind: run.kind };
}

/** Faces of one part decomposed on a global coordinate grid (for isometric fills). */
function partFaces(part, grid) {
  const faces = [];
  const [X, Y, Z] = grid.map((s, i) => s.filter((v) => v >= part.box.min[i] - EPS && v <= part.box.max[i] + EPS));
  for (let i = 0; i + 1 < X.length; i++) for (let j = 0; j + 1 < Y.length; j++) for (let k = 0; k + 1 < Z.length; k++) {
    const cmin = [X[i], Y[j], Z[k]], cmax = [X[i + 1], Y[j + 1], Z[k + 1]];
    const ctr = [(cmin[0] + cmax[0]) / 2, (cmin[1] + cmax[1]) / 2, (cmin[2] + cmax[2]) / 2];
    if (!occ(part, ctr)) continue;
    for (let a = 0; a < 3; a++) for (const s of [-1, 1]) {
      const probe = [...ctr]; probe[a] = (s > 0 ? cmax[a] : cmin[a]) + s * SAMPLE;
      if (occ(part, probe)) continue;
      const b = (a + 1) % 3, c = (a + 2) % 3;
      const corners = [];
      const va = s > 0 ? cmax[a] : cmin[a];
      for (const [sb, sc] of [[0, 0], [1, 0], [1, 1], [0, 1]]) {
        const p = [0, 0, 0]; p[a] = va; p[b] = sb ? cmax[b] : cmin[b]; p[c] = sc ? cmax[c] : cmin[c];
        corners.push(p);
      }
      faces.push({ part, axis: a, sign: s, at: va, corners, center: [ctr[0], ctr[1], ctr[2]].map((v, ii) => (ii === a ? va : v)) });
    }
  }
  return faces;
}

/** Ray from p in direction d (unit-ish): does it enter solid material of any part after t>EPS? */
function occluded(p, d, parts, skipPart = null) {
  for (const part of parts) {
    if (part === skipPart) continue;
    const iv = rayBox(p, d, part.box);
    if (!iv) continue;
    let intervals = [[Math.max(iv[0], EPS * 10), iv[1]]];
    if (intervals[0][1] - intervals[0][0] <= EPS * 10) continue;
    for (const c of part.cuts) {
      const ci = rayBox(p, d, c);
      if (!ci) continue;
      const next = [];
      for (const [a, b] of intervals) {
        if (ci[1] <= a + EPS || ci[0] >= b - EPS) { next.push([a, b]); continue; }
        if (ci[0] > a + EPS) next.push([a, ci[0]]);
        if (ci[1] < b - EPS) next.push([ci[1], b]);
      }
      intervals = next;
      if (!intervals.length) break;
    }
    if (intervals.some(([a, b]) => b - a > SAMPLE / 4)) return true;
  }
  return false;
}
function rayBox(p, d, b) {
  let t0 = -Infinity, t1 = Infinity;
  for (let i = 0; i < 3; i++) {
    if (Math.abs(d[i]) < 1e-12) {
      if (p[i] < b.min[i] - EPS || p[i] > b.max[i] + EPS) return null;
      continue;
    }
    let a = (b.min[i] - p[i]) / d[i], c = (b.max[i] - p[i]) / d[i];
    if (a > c) [a, c] = [c, a];
    t0 = Math.max(t0, a); t1 = Math.min(t1, c);
    if (t0 > t1) return null;
  }
  return [t0, t1];
}

// ───────────────────────────── views ─────────────────────────────
// A view maps model (x,y,z) → paper-plane (u,v) in model units; `toward` is the unit vector toward the viewer.
const VIEWS = {
  front: { kind: "ortho", title: "FRONT", proj: (p) => [p[0], -p[2]], toward: [0, 1, 0], uAxis: 0, vAxis: 2, vSign: -1, uSign: 1 },
  back: { kind: "ortho", title: "BACK", proj: (p) => [-p[0], -p[2]], toward: [0, -1, 0], uAxis: 0, vAxis: 2, vSign: -1, uSign: -1 },
  top: { kind: "ortho", title: "TOP", proj: (p) => [p[0], p[1]], toward: [0, 0, 1], uAxis: 0, vAxis: 1, vSign: 1, uSign: 1 },
  bottom: { kind: "ortho", title: "BOTTOM", proj: (p) => [p[0], -p[1]], toward: [0, 0, -1], uAxis: 0, vAxis: 1, vSign: -1, uSign: 1 },
  right: { kind: "ortho", title: "RIGHT SIDE", proj: (p) => [-p[1], -p[2]], toward: [1, 0, 0], uAxis: 1, vAxis: 2, vSign: -1, uSign: -1 },
  left: { kind: "ortho", title: "LEFT SIDE", proj: (p) => [p[1], -p[2]], toward: [-1, 0, 0], uAxis: 1, vAxis: 2, vSign: -1, uSign: 1 },
  iso: { kind: "iso", title: "ISOMETRIC", proj: (p) => [(p[0] - p[1]) * COS30, (p[0] + p[1]) * SIN30 - p[2]], toward: [1 / Math.sqrt(3), 1 / Math.sqrt(3), 1 / Math.sqrt(3)] },
  "iso-left": { kind: "iso", title: "ISOMETRIC", proj: (p) => [(-p[0] - p[1]) * COS30, (-p[0] + p[1]) * SIN30 - p[2]], toward: [-1 / Math.sqrt(3), 1 / Math.sqrt(3), 1 / Math.sqrt(3)] },
};

function seg2dIntersectT(a0, a1, b0, b1) {
  const dx = a1[0] - a0[0], dy = a1[1] - a0[1];
  const ex = b1[0] - b0[0], ey = b1[1] - b0[1];
  const den = dx * ey - dy * ex;
  if (Math.abs(den) < 1e-12) return null;
  const fx = b0[0] - a0[0], fy = b0[1] - a0[1];
  const t = (fx * ey - fy * ex) / den;
  const s = (fx * dy - fy * dx) / den;
  if (t <= 1e-9 || t >= 1 - 1e-9 || s < -1e-9 || s > 1 + 1e-9) return null;
  return t;
}

/**
 * Project all part edges into a view and resolve visibility by ray casting.
 * Returns { visible:[{u0,v0,u1,v1,axis,part,len}], hidden:[...] } in model units.
 */
function projectEdges(parts, view, opts = {}) {
  const all = [];
  for (const p of parts) for (const e of partEdges(p)) all.push(e);
  const proj = all.map((e) => ({ e, a: view.proj(e.p0), b: view.proj(e.p1) }));
  const visible = [], hidden = [];
  const toward = view.toward;
  for (const pe of proj) {
    const { e, a, b } = pe;
    const len2d = Math.hypot(b[0] - a[0], b[1] - a[1]);
    if (len2d < 1e-9) continue; // edge seen end-on
    const ts = [0, 1];
    for (const other of proj) {
      if (other === pe) continue;
      const t = seg2dIntersectT(a, b, other.a, other.b);
      if (t != null) ts.push(t);
    }
    ts.sort((x, y) => x - y);
    let runStart = 0, runVisible = null;
    const flush = (t0, t1, vis) => {
      if (t1 - t0 < 1e-9) return;
      const p0 = lerp3(e.p0, e.p1, t0), p1 = lerp3(e.p0, e.p1, t1);
      const A = view.proj(p0), B = view.proj(p1);
      const seg = { u0: A[0], v0: A[1], u1: B[0], v1: B[1], axis: e.axis, part: e.part, len: Math.abs((t1 - t0) * (e.p1[e.axis] - e.p0[e.axis])), kind: e.kind, p0, p1 };
      (vis ? visible : hidden).push(seg);
    };
    for (let i = 0; i + 1 < ts.length; i++) {
      if (ts[i + 1] - ts[i] < 1e-9) continue;
      const tm = (ts[i] + ts[i + 1]) / 2;
      const pm = lerp3(e.p0, e.p1, tm);
      // nudge the sample a hair toward the viewer so a point on the surface of its own part is not self-occluded
      const vis = !occluded(pm, toward, parts, null);
      if (runVisible === null) { runVisible = vis; runStart = ts[i]; }
      else if (vis !== runVisible) { flush(runStart, ts[i], runVisible); runVisible = vis; runStart = ts[i]; }
    }
    if (runVisible !== null) flush(runStart, ts[ts.length - 1], runVisible);
  }
  return { visible, hidden: opts.hidden === false ? [] : hidden };
}
const lerp3 = (a, b, t) => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];

function visibleFaces(parts, view) {
  if (view.kind !== "iso") return [];
  const grid = coordSets(parts);
  const faces = [];
  for (const p of parts) for (const f of partFaces(p, grid)) {
    const n = [0, 0, 0]; n[f.axis] = f.sign;
    if (n[0] * view.toward[0] + n[1] * view.toward[1] + n[2] * view.toward[2] <= 0) continue;
    // drop faces fully hidden (centre + corners all occluded)
    const samples = [f.center, ...f.corners.map((c) => lerp3(c, f.center, 0.02))];
    if (samples.every((s) => occluded(s, view.toward, parts, null))) continue;
    faces.push(f);
  }
  faces.sort((a, b) => depthOf(a.center, view) - depthOf(b.center, view));
  return faces;
}
const depthOf = (p, view) => p[0] * view.toward[0] + p[1] * view.toward[1] + p[2] * view.toward[2];

function extent2d(segs) {
  let u0 = Infinity, v0 = Infinity, u1 = -Infinity, v1 = -Infinity;
  for (const s of segs) {
    u0 = Math.min(u0, s.u0, s.u1); u1 = Math.max(u1, s.u0, s.u1);
    v0 = Math.min(v0, s.v0, s.v1); v1 = Math.max(v1, s.v0, s.v1);
  }
  return { u0, v0, u1, v1 };
}
function extentOfBoxes(parts, view) {
  let u0 = Infinity, v0 = Infinity, u1 = -Infinity, v1 = -Infinity;
  for (const p of parts) for (const c of boxCorners(p.box)) {
    const [u, v] = view.proj(c);
    u0 = Math.min(u0, u); u1 = Math.max(u1, u); v0 = Math.min(v0, v); v1 = Math.max(v1, v);
  }
  return { u0, v0, u1, v1 };
}
function boxCorners(b) {
  const out = [];
  for (const x of [b.min[0], b.max[0]]) for (const y of [b.min[1], b.max[1]]) for (const z of [b.min[2], b.max[2]]) out.push([x, y, z]);
  return out;
}

// ───────────────────────────── SVG builder ─────────────────────────────
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const attrs = (o) => Object.entries(o).filter(([, v]) => v != null && v !== "").map(([k, v]) => ` ${k}="${esc(typeof v === "number" ? r3(v) : v)}"`).join("");

class Svg {
  constructor(sheet, slug, plain = false) {
    this.w = sheet.w; this.h = sheet.h; this.slug = slug; this.plain = plain;
    this.body = []; this.defs = new Set(); this.textCount = 0;
  }
  raw(s) { this.body.push(s); }
  open(tag, a = {}) { this.body.push(`<${tag}${attrs(a)}>`); }
  close(tag) { this.body.push(`</${tag}>`); }
  line(x0, y0, x1, y1, a = {}) { this.body.push(`<line${attrs({ x1: x0, y1: y0, x2: x1, y2: y1, ...a })}/>`); }
  poly(pts, a = {}) { this.body.push(`<polygon${attrs({ points: pts.map(([x, y]) => `${r3(x)},${r3(y)}`).join(" "), ...a })}/>`); }
  polyline(pts, a = {}) { this.body.push(`<polyline${attrs({ points: pts.map(([x, y]) => `${r3(x)},${r3(y)}`).join(" "), fill: "none", ...a })}/>`); }
  rect(x, y, w, h, a = {}) { this.body.push(`<rect${attrs({ x, y, width: w, height: h, ...a })}/>`); }
  circle(cx, cy, r, a = {}) { this.body.push(`<circle${attrs({ cx, cy, r, ...a })}/>`); }
  text(x, y, s, a = {}) {
    if (this.plain) return;
    this.textCount++;
    const size = a["font-size"] ?? TXT.dim;
    this.body.push(`<text${attrs({ x, y, "font-size": size, "font-family": FONT, fill: INK, ...a })}>${esc(s)}</text>`);
  }
  def(id, markup) { this.defs.add(`<${markup.startsWith("<") ? markup.slice(1) : markup}`); this._defIds = this._defIds || new Set(); this._defIds.add(id); }
  hasDef(id) { return this._defIds?.has(id); }
  toString() {
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${this.w}mm" height="${this.h}mm" viewBox="0 0 ${this.w} ${this.h}" data-draft="1" data-sheet-mm="${this.w} ${this.h}" font-family="${FONT}">\n` +
      `<defs>\n${[...this.defs].join("\n")}\n</defs>\n<rect x="0" y="0" width="${this.w}" height="${this.h}" fill="#fff"/>\n` +
      this.body.join("\n") + "\n</svg>\n";
  }
}

const textWidth = (s, size) => String(s).length * size * CHAR_W;

// ───────────────────────────── primitives ─────────────────────────────
function arrowHead(svg, tip, dir, a = {}) {
  // dir points from tip back along the line
  const L = 3, W = 0.5;
  const n = [-dir[1], dir[0]];
  svg.poly([
    tip,
    [tip[0] + dir[0] * L + n[0] * W, tip[1] + dir[1] * L + n[1] * W],
    [tip[0] + dir[0] * L - n[0] * W, tip[1] + dir[1] * L - n[1] * W],
  ], { class: "arrow", fill: DIM_INK, stroke: "none", ...a });
}

/**
 * Generic dimension. A, B: paper points of the measured corners. ext: unit vector
 * pointing away from the object (extension direction). off: distance from A/B to
 * the dimension line along ext. label: string. meta: data attributes for checks.
 */
function drawDim(svg, A, B, ext, off, label, meta = {}, textSide = 0) {
  if (svg.plain) return;
  const gap = 1.2, past = 2;
  const P = [A[0] + ext[0] * off, A[1] + ext[1] * off];
  const Q = [B[0] + ext[0] * off, B[1] + ext[1] * off];
  const span = Math.hypot(Q[0] - P[0], Q[1] - P[1]);
  if (span < 1e-6) return;
  const dir = [(Q[0] - P[0]) / span, (Q[1] - P[1]) / span];
  svg.open("g", { class: "dim", ...meta });
  // extension lines
  for (const [S, T] of [[A, P], [B, Q]]) {
    svg.line(S[0] + ext[0] * gap, S[1] + ext[1] * gap, T[0] + ext[0] * past, T[1] + ext[1] * past, { class: "ext", stroke: DIM_INK, "stroke-width": LW.dim });
  }
  const size = TXT.dim;
  const tw = textWidth(label, size);
  const horizontal = Math.abs(dir[1]) < 1e-6;
  const vertical = Math.abs(dir[0]) < 1e-6;
  const tight = span < 2 * 3 + 2; // two arrowheads + breathing room
  const mid = [(P[0] + Q[0]) / 2, (P[1] + Q[1]) / 2];
  if (tight) {
    // arrows outside pointing in, line extended, text centred and pushed away from the object
    const e = 4.5;
    svg.line(P[0] - dir[0] * e, P[1] - dir[1] * e, Q[0] + dir[0] * e, Q[1] + dir[1] * e, { class: "dim-line", stroke: DIM_INK, "stroke-width": LW.dim });
    arrowHead(svg, P, [-dir[0], -dir[1]]);
    arrowHead(svg, Q, dir);
    if (textSide) {
      // text beyond the chain end, clear of every extension line
      const E = textSide < 0 ? P : Q, sgn = textSide < 0 ? -1 : 1;
      const t = [E[0] + sgn * dir[0] * (e + 1), E[1] + sgn * dir[1] * (e + 1)];
      if (vertical) svg.text(t[0], t[1] + (sgn * dir[1] > 0 ? size * 0.85 : -size * 0.15), label, { "font-size": size, "text-anchor": "middle" });
      else svg.text(t[0], t[1] + size * 0.35, label, { "font-size": size, "text-anchor": sgn * dir[0] > 0 ? "start" : "end" });
    } else {
      // mid-chain: clear the neighbours' extension-line overshoot (2 mm past the line)
      const stagger = 2.5 + (meta["data-stagger"] ? Number(meta["data-stagger"]) * (size + 0.6) : 0);
      const clear = (tw / 2) * Math.abs(ext[0]) + (size / 2) * Math.abs(ext[1]) + 0.8 + stagger;
      const c = [mid[0] + ext[0] * clear, mid[1] + ext[1] * clear];
      svg.text(c[0], c[1] + size * 0.35, label, { "font-size": size, "text-anchor": "middle" });
    }
  } else if (vertical) {
    // break the line and set horizontal text in the gap
    const half = size * 0.75;
    svg.line(P[0], P[1], mid[0], mid[1] - half * Math.sign(mid[1] - P[1] || 1), { class: "dim-line", stroke: DIM_INK, "stroke-width": LW.dim });
    svg.line(mid[0], mid[1] + half * Math.sign(Q[1] - mid[1] || 1), Q[0], Q[1], { class: "dim-line", stroke: DIM_INK, "stroke-width": LW.dim });
    arrowHead(svg, P, dir);
    arrowHead(svg, Q, [-dir[0], -dir[1]]);
    svg.text(mid[0], mid[1] + size * 0.35, label, { "font-size": size, "text-anchor": "middle" });
  } else {
    svg.line(P[0], P[1], Q[0], Q[1], { class: "dim-line", stroke: DIM_INK, "stroke-width": LW.dim });
    arrowHead(svg, P, dir);
    arrowHead(svg, Q, [-dir[0], -dir[1]]);
    // text offset away from the object, clear of the line for any angle
    const clear = (tw / 2) * Math.abs(ext[0]) + (size / 2) * Math.abs(ext[1]) + 0.8;
    const c = [mid[0] + ext[0] * clear, mid[1] + ext[1] * clear];
    svg.text(c[0], c[1] + size * 0.35, label, { "font-size": size, "text-anchor": "middle" });
  }
  svg.close("g");
}

function balloon(svg, at, anchor, n) {
  if (svg.plain) return;
  const r = 3.2;
  const d = Math.hypot(anchor[0] - at[0], anchor[1] - at[1]) || 1;
  const ux = (anchor[0] - at[0]) / d, uy = (anchor[1] - at[1]) / d;
  svg.open("g", { class: "balloon", "data-item": n });
  svg.line(at[0] + ux * r, at[1] + uy * r, anchor[0], anchor[1], { class: "leader", stroke: DIM_INK, "stroke-width": LW.leader });
  svg.circle(anchor[0], anchor[1], 0.6, { fill: DIM_INK, stroke: "none" });
  svg.circle(at[0], at[1], r, { fill: "#fff", stroke: DIM_INK, "stroke-width": LW.dim });
  svg.text(at[0], at[1] + TXT.balloon * 0.35, String(n), { "font-size": TXT.balloon, "text-anchor": "middle", "font-weight": "600" });
  svg.close("g");
}

function hatchDef(svg, id, color = INK) {
  if (svg.hasDef(id)) return;
  svg.def(id, `<pattern id="${id}" width="2.5" height="2.5" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><line x1="0" y1="0" x2="0" y2="2.5" stroke="${color}" stroke-width="${LW.hatch}"/></pattern>`);
}

function gridDef(svg, id, minor, major) {
  if (svg.hasDef(id)) return;
  const n = Math.round(major / minor);
  let lines = "";
  for (let i = 1; i < n; i++) lines += `<line x1="${r3(i * minor)}" y1="0" x2="${r3(i * minor)}" y2="${r3(major)}" stroke="${GRID_MINOR}" stroke-width="0.12"/><line x1="0" y1="${r3(i * minor)}" x2="${r3(major)}" y2="${r3(i * minor)}" stroke="${GRID_MINOR}" stroke-width="0.12"/>`;
  svg.def(id, `<pattern id="${id}" width="${r3(major)}" height="${r3(major)}" patternUnits="userSpaceOnUse">${lines}<path d="M0 0H${r3(major)}V${r3(major)}" fill="none" stroke="${GRID_MAJOR}" stroke-width="0.2"/></pattern>`);
}

// ───────────────────────────── placed views ─────────────────────────────
/** A placed view: paper = origin + (u,v) * scale. */
let VIEW_SEQ = 0;
function place(view, parts, scale, ox, oy, name) {
  return { vid: ++VIEW_SEQ, view, parts, scale, ox, oy, name: name || Object.keys(VIEWS).find((k) => VIEWS[k] === view) || "view",
    P: (u, v) => [ox + u * scale, oy + v * scale], P3: (p) => { const [u, v] = view.proj(p); return [ox + u * scale, oy + v * scale]; } };
}

/** Render one placed view: faces, hatch, hidden, visible, extents. */
function renderView(svg, pv, opts = {}) {
  const { view, parts, scale } = pv;
  const { section = null, breakPlanes = [], hiddenLines = view.kind === "ortho", grid = false, units = "in" } = opts;
  const tone = (part, axis) => {
    const t = TONES[part.tone] || TONES.light;
    if (axis === 2) return t.top;
    if (axis === 1) return t.front;
    return t.right;
  };
  svg.open("g", { class: "view", "data-view": pv.name, "data-vid": pv.vid, "data-kind": view.kind, "data-scale": scale, "data-origin": `${r3(pv.ox)} ${r3(pv.oy)}` });
  const ext = extentOfBoxes(parts, view);
  if (grid && view.kind === "ortho") {
    const minor = (units === "in" ? 0.25 : 10) * scale, major = (units === "in" ? 1 : 50) * scale;
    if (minor >= 1.0) {
      const id = `${svg.slug}-${pv.name}-grid`;
      gridDef(svg, id, minor, major);
      const pad = 6;
      const [x0, y0] = pv.P(ext.u0, ext.v0), [x1, y1] = pv.P(ext.u1, ext.v1);
      // align the pattern to the model origin so squares count from the object corner
      svg.rect(x0 - pad, y0 - pad, x1 - x0 + 2 * pad, y1 - y0 + 2 * pad, { fill: `url(#${id})`, stroke: "none", class: "grid", transform: `translate(${r3(((x0 - pad) - x0) % major)} 0)` });
    }
  }
  // faces (isometric)
  if (view.kind === "iso") {
    for (const f of visibleFaces(parts, view)) {
      const pts = f.corners.map((c) => pv.P3(c));
      const fill = tone(f.part, f.axis);
      svg.poly(pts, { class: "face", fill, stroke: fill, "stroke-width": 0.15, "data-part": f.part.id });
    }
  }
  // section hatch: cells on the cutting plane
  if (section) {
    const id = `${svg.slug}-hatch`;
    hatchDef(svg, id);
    const grid3 = coordSets(parts);
    for (const part of parts) {
      for (const f of partFaces(part, grid3)) {
        if (f.axis !== section.axis || !nearly(f.at, section.at, 1e-6) || f.sign !== section.viewerSign) continue;
        const pts = f.corners.map((c) => pv.P3(c));
        svg.poly(pts, { class: "cut-face", fill: `url(#${id})`, stroke: "none", "data-part": part.id });
      }
    }
  }
  const { visible, hidden } = projectEdges(parts, view);
  const onPlane = (s, pl) => nearly(s.p0[pl.axis], pl.at, 1e-6) && nearly(s.p1[pl.axis], pl.at, 1e-6);
  for (const s of visible) {
    if (breakPlanes.some((bp) => onPlane(s, bp))) s._brk = true;
    if (section && onPlane(s, section)) s._cut = true;
  }
  if (hiddenLines) {
    for (const s of hidden) {
      const [x0, y0] = pv.P(s.u0, s.v0), [x1, y1] = pv.P(s.u1, s.v1);
      svg.line(x0, y0, x1, y1, { class: "hid", stroke: HID_INK, "stroke-width": LW.hid, "stroke-dasharray": "1.6 0.8", "data-part": s.part.id, "data-axis": AXIS_NAME[s.axis], "data-len": s.len });
    }
  }
  for (const s of visible) {
    const [x0, y0] = pv.P(s.u0, s.v0), [x1, y1] = pv.P(s.u1, s.v1);
    const brk = s._brk;
    const cut = section && s._cut;
    svg.line(x0, y0, x1, y1, { class: brk ? "brk" : cut ? "cut" : "obj", stroke: INK, "stroke-width": brk ? LW.brk : cut ? LW.cut : LW.obj, "stroke-linecap": "round", "data-part": s.part.id, "data-axis": AXIS_NAME[s.axis], "data-len": s.len });
  }
  // invisible extents for the proportion check
  const partial = new Set(hiddenLines ? [] : hidden.map((s) => s.part.id));
  for (const part of parts) {
    const e = extentOfBoxes([part], view);
    const [x0, y0] = pv.P(e.u0, e.v0), [x1, y1] = pv.P(e.u1, e.v1);
    svg.rect(x0, y0, x1 - x0, y1 - y0, { class: "extent", fill: "none", stroke: "none", "data-part": part.id, "data-size": `${r3(e.u1 - e.u0)} ${r3(e.v1 - e.v0)}`, "data-partial": partial.has(part.id) ? "1" : null });
  }
  svg.close("g");
  return { ext, visible, hidden };
}

/** Weighted centroid of a part's visible segments (paper coords), or null. */
function anchorFor(pv, visible, part) {
  let sx = 0, sy = 0, sw = 0;
  for (const s of visible) {
    if (s.part !== part) continue;
    const w = Math.hypot(s.u1 - s.u0, s.v1 - s.v0);
    sx += ((s.u0 + s.u1) / 2) * w; sy += ((s.v0 + s.v1) / 2) * w; sw += w;
  }
  if (sw < 1e-9) return null;
  return pv.P(sx / sw, sy / sw);
}

/** Place balloons around a view on the allowed sides; returns nothing, draws. */
function drawBalloons(svg, pv, visible, parts, sides, extentPaper, offset = 9) {
  if (svg.plain) return;
  const items = new Map();
  for (const part of parts) {
    if (items.has(part.item)) continue;
    const a = anchorFor(pv, visible, part);
    if (a) items.set(part.item, { item: part.item, anchor: a });
  }
  const { x0, y0, x1, y1 } = extentPaper;
  const bySide = { top: [], bottom: [], left: [], right: [] };
  for (const it of items.values()) {
    const cands = sides.map((s) => {
      const d = s === "top" ? it.anchor[1] - y0 : s === "bottom" ? y1 - it.anchor[1] : s === "left" ? it.anchor[0] - x0 : x1 - it.anchor[0];
      return { s, d };
    }).sort((p, q) => p.d - q.d);
    bySide[cands[0].s].push(it);
  }
  const step = 8;
  for (const side of Object.keys(bySide)) {
    const list = bySide[side];
    if (!list.length) continue;
    const horizontal = side === "top" || side === "bottom";
    list.sort((p, q) => (horizontal ? p.anchor[0] - q.anchor[0] : p.anchor[1] - q.anchor[1]));
    const fixed = horizontal ? (side === "top" ? y0 - offset : y1 + offset) : (side === "left" ? x0 - offset : x1 + offset);
    // spread positions with a minimum step, then centre the run on its anchors
    const desired = list.map((it) => (horizontal ? it.anchor[0] : it.anchor[1]));
    const pos = [...desired];
    for (let i = 1; i < pos.length; i++) pos[i] = Math.max(pos[i], pos[i - 1] + step);
    for (let i = pos.length - 2; i >= 0; i--) pos[i] = Math.min(pos[i], pos[i + 1] - step);
    const shift = (desired.reduce((a, b) => a + b, 0) - pos.reduce((a, b) => a + b, 0)) / pos.length;
    list.forEach((it, i) => {
      const at = horizontal ? [pos[i] + shift, fixed] : [fixed, pos[i] + shift];
      balloon(svg, at, it.anchor, it.item);
    });
  }
}

// ───────────────────────────── sheet furniture ─────────────────────────────
function sheetFrame(svg, model, info) {
  if (svg.plain) return;
  const { w, h } = svg;
  svg.rect(MARGIN, MARGIN, w - 2 * MARGIN, h - 2 * MARGIN, { fill: "none", stroke: INK, "stroke-width": LW.border, class: "border" });
  // title block, bottom right
  const bx = w - MARGIN - TITLE_BLOCK.w, by = h - MARGIN - TITLE_BLOCK.h;
  const bw = TITLE_BLOCK.w, bh = TITLE_BLOCK.h;
  svg.open("g", { class: "title-block" });
  svg.rect(bx, by, bw, bh, { fill: "#fff", stroke: INK, "stroke-width": LW.border });
  const rows = [by, by + 9, by + 18.5, by + bh];
  svg.line(bx, rows[1], bx + bw, rows[1], { stroke: INK, "stroke-width": LW.dim });
  svg.line(bx, rows[2], bx + bw, rows[2], { stroke: INK, "stroke-width": LW.dim });
  const cell = (x, y, wdt, label, value, big = false) => {
    svg.text(x + 1.5, y + 2.8, label, { "font-size": 2, fill: "#555" });
    svg.text(x + 1.5, y + (big ? 8 : 7.3), value, { "font-size": big ? TXT.blockTitle : TXT.block, "font-weight": big ? "700" : "500" });
  };
  cell(bx, rows[0], bw, "PROJECT", model.project, true);
  cell(bx, rows[1], bw, "SHEET TITLE", info.title);
  const cols = [bx, bx + 34, bx + 58, bx + 78, bx + 98, bx + bw];
  for (let i = 1; i < cols.length - 1; i++) svg.line(cols[i], rows[2], cols[i], rows[3], { stroke: INK, "stroke-width": LW.dim });
  cell(cols[0], rows[2], 0, "SCALE", info.scaleLabel);
  cell(cols[1], rows[2], 0, "UNITS", model.units === "mm" ? "mm" : "inches");
  cell(cols[2], rows[2], 0, "SHEET", `${info.n} OF ${info.m}`);
  cell(cols[3], rows[2], 0, "DATE", model.date);
  cell(cols[4], rows[2], 0, "REV", model.revision);
  svg.close("g");
  // projection symbol + drawn-by, left of the title block
  const sx = bx - 40, sy = by + 4;
  svg.open("g", { class: "projection-symbol" });
  svg.text(sx, sy + 2.2, "THIRD ANGLE PROJECTION", { "font-size": 2.2, fill: "#555" });
  svg.poly([[sx + 1, sy + 6], [sx + 9, sy + 6], [sx + 9, sy + 14], [sx + 1, sy + 11]].map(([x, y]) => [x, y]), { fill: "none", stroke: INK, "stroke-width": LW.dim });
  svg.circle(sx + 17, sy + 10, 4, { fill: "none", stroke: INK, "stroke-width": LW.dim });
  svg.circle(sx + 17, sy + 10, 2, { fill: "none", stroke: INK, "stroke-width": LW.dim });
  svg.text(sx, sy + 20, `DRAWN: ${model.author}`, { "font-size": 2.2, fill: "#555" });
  svg.close("g");
  // scale bar, bottom left
  if (info.scale) scaleBar(svg, MARGIN + 4, h - MARGIN - 8, info.scale, model.units, info.scaleLabel);
  // reference + notes, above the scale bar, left
  let ny = h - MARGIN - TITLE_BLOCK.h + 2;
  const notes = [...(info.notes || [])];
  if (model.reference?.path && info.n === 1) notes.unshift(`REFERENCE: ${model.reference.path}`);
  const noteWidth = sx - 6 - (MARGIN + 4);
  const lines = [];
  for (const n of notes) {
    const maxChars = Math.floor(noteWidth / (2.2 * CHAR_W));
    let rest = String(n);
    while (rest.length > maxChars) {
      let cut = rest.lastIndexOf(" ", maxChars);
      if (cut < maxChars / 2) cut = maxChars;
      lines.push(rest.slice(0, cut).trim()); rest = rest.slice(cut).trim();
    }
    lines.push(rest);
  }
  for (const l of lines.slice(0, 6)) { svg.text(MARGIN + 4, ny + 2.5, l, { "font-size": 2.2, fill: "#333" }); ny += 3.2; }
}

function scaleBar(svg, x, y, scale, units, label) {
  const cands = units === "in" ? [1, 2, 3, 6, 12, 24, 36, 48] : [10, 20, 50, 100, 200, 500, 1000];
  let L = cands[0];
  for (const c of cands) if (c * scale <= 60) L = c;
  const px = L * scale;
  const divisions = units === "in" ? (L >= 12 ? L / 6 : L) : 5;
  svg.open("g", { class: "scale-bar" });
  svg.line(x, y, x + px, y, { stroke: INK, "stroke-width": 0.6 });
  for (let i = 0; i <= divisions; i++) {
    const tx = x + (px * i) / divisions;
    svg.line(tx, y - 1.5, tx, y + 1.5, { stroke: INK, "stroke-width": 0.35 });
  }
  svg.text(x, y + 5, `${fmt(L, units)} ${units} = ${r3(px)} mm on paper at ${label}`, { "font-size": 2.4 });
  svg.close("g");
}

function viewTitle(svg, x, y, title, sub) {
  svg.text(x, y, title, { "font-size": TXT.title, "font-weight": "700", "text-anchor": "middle", class: "view-title" });
  if (sub) svg.text(x, y + 4, sub, { "font-size": 2.6, "text-anchor": "middle", fill: "#555" });
}

// ───────────────────────────── scale choice ─────────────────────────────
function scaleList(units) {
  const f = units === "in" ? MM_PER_IN : 1;
  return (units === "in" ? SCALES_IN : SCALES_MM).map((n) => ({ n, s: f / n, label: `1:${n}` }));
}
function pickScale(units, needW, needH, availW, availH, explicit) {
  const list = scaleList(units);
  if (explicit) {
    const m = String(explicit).match(/1\s*:\s*(\d+(?:\.\d+)?)/);
    const n = m ? Number(m[1]) : Number(explicit);
    const f = units === "in" ? MM_PER_IN : 1;
    return { n, s: f / n, label: `1:${n}` };
  }
  for (const sc of list) if (needW(sc.s) <= availW && needH(sc.s) <= availH) return sc;
  return list[list.length - 1];
}

// ───────────────────────────── dimensions on views ─────────────────────────────
const SIDE_DEFAULT = {
  front: { 0: "bottom", 2: "left" }, back: { 0: "bottom", 2: "left" },
  top: { 0: "top", 1: "left" }, bottom: { 0: "bottom", 1: "left" },
  right: { 1: "bottom", 2: "right" }, left: { 1: "bottom", 2: "left" },
};
const ROW0 = 10, ROW_STEP = 11;

function reserve(rows, isBottom = false) {
  if (!rows) return isBottom ? 9 : 0;
  return ROW0 + ROW_STEP * (rows - 1) + 5 + (isBottom ? 9 : 0);
}

/** Build dimension specs for a view: model dims filtered to this view plus overall dims. */
function specsFor(viewName, view, model, parts, overall = true) {
  const bb = assemblyBox(parts);
  const specs = [];
  const maxRow = {};
  for (const d of model.dims) {
    if (d.view !== viewName) continue;
    const axis = AXIS[d.axis];
    if (axis == null || (axis !== view.uAxis && axis !== view.vAxis)) continue;
    const side = d.side || SIDE_DEFAULT[viewName]?.[axis] || "bottom";
    const row = d.row || 1;
    specs.push({ axis, from: num(d.from), to: num(d.to), row, side, label: d.label, note: d.note });
    maxRow[side] = Math.max(maxRow[side] || 0, row);
  }
  if (overall) {
    for (const axis of [view.uAxis, view.vAxis]) {
      const side = SIDE_DEFAULT[viewName]?.[axis];
      if (!side) continue;
      if (viewName === "top" && axis === 0) continue; // width is dimensioned on the front
      if (viewName === "right" && axis === 2) continue; // height is dimensioned on the front
      const row = (maxRow[side] || 0) + 1;
      specs.push({ axis, from: bb.min[axis], to: bb.max[axis], row, side, overall: true });
      maxRow[side] = row;
    }
  }
  return { specs, maxRow };
}

function drawSpecs(svg, pv, ext, specs, units) {
  const { view, scale } = pv;
  // tight dims (span too small for arrows + text) put their text beyond the chain end when they are
  // first/last in their row, and stagger away from the object when they sit mid-chain
  const groups = new Map();
  for (const sp of specs) { const k = `${sp.side}|${sp.row}`; if (!groups.has(k)) groups.set(k, []); groups.get(k).push(sp); }
  for (const list of groups.values()) {
    list.sort((a, b) => Math.min(a.from, a.to) - Math.min(b.from, b.to));
    let stagger = 0;
    list.forEach((sp, i) => {
      const tight = Math.abs(sp.to - sp.from) * scale < 8;
      sp.textSide = 0; sp.stagger = 0;
      if (!tight) { stagger = 0; return; }
      if (i === 0) sp.textSide = -1; else if (i === list.length - 1) sp.textSide = 1; else { sp.stagger = stagger % 2; stagger++; }
    });
  }
  for (const sp of specs) {
    const horizontal = sp.axis === view.uAxis;
    const sign = horizontal ? view.uSign : view.vSign;
    const a = sp.from * sign, b = sp.to * sign;
    let A, B, extv;
    if (horizontal) {
      const vEdge = sp.side === "top" ? ext.v0 : ext.v1;
      A = pv.P(a, vEdge); B = pv.P(b, vEdge); extv = sp.side === "top" ? [0, -1] : [0, 1];
    } else {
      const uEdge = sp.side === "left" ? ext.u0 : ext.u1;
      A = pv.P(uEdge, a); B = pv.P(uEdge, b); extv = sp.side === "left" ? [-1, 0] : [1, 0];
    }
    const value = Math.abs(sp.to - sp.from);
    const off = ROW0 + ROW_STEP * (sp.row - 1);
    drawDim(svg, A, B, extv, off, sp.label || fmt(value, units), {
      "data-view": pv.name, "data-vid": pv.vid, "data-axis": AXIS_NAME[sp.axis], "data-row": sp.row, "data-side": sp.side,
      "data-from": r3(Math.min(sp.from, sp.to)), "data-to": r3(Math.max(sp.from, sp.to)), "data-value": r3(value), "data-overall": sp.overall ? "1" : null, "data-stagger": sp.stagger || null,
    }, sp.textSide || 0);
  }
}

function drawOverlays(svg, pv, model, variant) {
  if (svg.plain) return;
  const { view } = pv;
  for (const o of model.overlays) {
    if (o.view !== pv.name) continue;
    if (o.variant && o.variant !== variant) continue;
    const toUV = (c) => {
      const p = [0, 0, 0];
      p[view.uAxis] = num(c[0]); p[view.vAxis] = num(c[1]);
      return view.proj(p);
    };
    if (o.type === "circle") {
      const [u, v] = toUV(o.center);
      const [cx, cy] = pv.P(u, v);
      const r = num(o.r) * pv.scale;
      svg.open("g", { class: "overlay" });
      svg.circle(cx, cy, r, { fill: "none", stroke: INK, "stroke-width": LW.hid, "stroke-dasharray": o.reference === false ? null : "2 1" });
      svg.line(cx - r - 3, cy, cx + r + 3, cy, { stroke: INK, "stroke-width": LW.center, "stroke-dasharray": "6 1.5 1.5 1.5", class: "center" });
      svg.line(cx, cy - r - 3, cx, cy + r + 3, { stroke: INK, "stroke-width": LW.center, "stroke-dasharray": "6 1.5 1.5 1.5", class: "center" });
      if (o.label) svg.text(cx, cy - 2.5, o.label, { "font-size": TXT.dim, "text-anchor": "middle" });
      if (o.label2) svg.text(cx, cy + 6, o.label2, { "font-size": TXT.dim, "text-anchor": "middle" });
      svg.close("g");
    } else if (o.type === "label") {
      const [u, v] = toUV(o.at);
      const [x, y] = pv.P(u, v);
      svg.open("g", { class: "overlay" });
      if (o.leader_to) {
        const [lu, lv] = toUV(o.leader_to);
        const [lx, ly] = pv.P(lu, lv);
        svg.line(x, y, lx, ly, { stroke: DIM_INK, "stroke-width": LW.leader, class: "leader" });
        svg.circle(lx, ly, 0.6, { fill: DIM_INK });
      }
      svg.text(x, y + TXT.dim * 0.35, o.text, { "font-size": TXT.dim, "text-anchor": o.anchor || "middle" });
      svg.close("g");
    }
  }
}

function drawCuttingPlanes(svg, placed, model) {
  if (svg.plain) return;
  for (const sec of model.sections) {
    const host = sec.axis === 1 ? placed.top : placed.front;
    if (!host) continue;
    const { view } = host;
    const ext = host.ext;
    let A, B, dirArrow;
    const res = host.reserves || {};
    const padL = 7 + (res.left || 0), padR = 7 + (res.right || 0), padT = 7 + (res.top || 0), padB = 7 + (res.bottom || 0);
    if (sec.axis === 1 && host === placed.top) {
      const v = sec.at * view.vSign;
      A = host.P(ext.u0 - padL / host.scale, v); B = host.P(ext.u1 + padR / host.scale, v);
      dirArrow = [0, -view.vSign * (sec.look)]; // look -1 (viewer at +y) → arrows toward −y (up on paper)
    } else if (sec.axis === 0) {
      const u = sec.at * view.uSign;
      A = host.P(u, ext.v0 - padT / host.scale); B = host.P(u, ext.v1 + padB / host.scale);
      dirArrow = [view.uSign * sec.look, 0];
    } else if (sec.axis === 2) {
      const v = sec.at * view.vSign;
      A = host.P(ext.u0 - padL / host.scale, v); B = host.P(ext.u1 + padR / host.scale, v);
      dirArrow = [0, view.vSign * sec.look];
    } else continue;
    svg.open("g", { class: "cutting-plane" });
    const along = [Math.sign(B[0] - A[0]), Math.sign(B[1] - A[1])];
    // ASME-style ends-only cutting-plane line: two short bars outside the dimension rows
    for (const E of [A, B]) {
      const inward = E === A ? along : [-along[0], -along[1]];
      svg.line(E[0], E[1], E[0] + inward[0] * 6, E[1] + inward[1] * 6, { stroke: INK, "stroke-width": 0.6, "stroke-dasharray": "5 1.2 1.2 1.2" });
    }
    for (const E of [A, B]) {
      const tip = [E[0] + dirArrow[0] * 5, E[1] + dirArrow[1] * 5];
      svg.line(E[0], E[1], tip[0], tip[1], { stroke: INK, "stroke-width": 0.5 });
      arrowHead(svg, tip, [-dirArrow[0], -dirArrow[1]]);
      const lx = E[0] + dirArrow[0] * 8 + (dirArrow[0] === 0 ? (E === A ? -2.5 : 2.5) : 0);
      const ly = E[1] + dirArrow[1] * 8 + (dirArrow[1] === 0 ? (E === A ? -2 : 4) : 1.2);
      svg.text(lx, ly, sec.name, { "font-size": TXT.label, "font-weight": "700", "text-anchor": "middle" });
    }
    svg.close("g");
  }
}

// ───────────────────────────── sheets ─────────────────────────────
function area(svg) {
  return { x0: MARGIN + 4, y0: MARGIN + 4, x1: svg.w - MARGIN - 4, y1: svg.h - MARGIN - TITLE_BLOCK.h - 5 };
}

function orthoSheet(model, parts, opts) {
  const title = opts.title || "ORTHOGRAPHIC SET";
  return {
    title,
    draw(svg) {
      const ar = area(svg);
      const bb = assemblyBox(parts);
      const [W, D, H] = boxDims(bb);
      const F = specsFor("front", VIEWS.front, model, parts);
      const T = specsFor("top", VIEWS.top, model, parts);
      const R = specsFor("right", VIEWS.right, model, parts);
      const resL = Math.max(reserve(F.maxRow.left || 0), reserve(T.maxRow.left || 0));
      const resB = Math.max(reserve(F.maxRow.bottom || 0, true), reserve(R.maxRow.bottom || 0, true));
      const gapH = 12 + reserve(R.maxRow.left || 0) + 10; // balloons on the front's right
      const gapV = 12 + reserve(F.maxRow.top || 0) + 10 + reserve(T.maxRow.bottom || 0, true);
      const resR = 10 + reserve(R.maxRow.right || 0);
      const resT = 10 + reserve(T.maxRow.top || 0);
      const needW = (s) => resL + W * s + gapH + D * s + resR;
      const needH = (s) => resT + D * s + gapV + H * s + resB;
      const sc = pickScale(model.units, needW, needH, ar.x1 - ar.x0, ar.y1 - ar.y0, opts.scale);
      const s = sc.s;
      const slackX = Math.max(0, (ar.x1 - ar.x0) - needW(s));
      const slackY = Math.max(0, (ar.y1 - ar.y0) - needH(s));
      const fx0 = ar.x0 + resL + Math.min(slackX / 2, 20);
      const fy1 = ar.y1 - resB - Math.min(slackY / 2, 10);
      const front = place(VIEWS.front, parts, s, fx0 - bb.min[0] * s, fy1 + bb.min[2] * s, "front");
      const top = place(VIEWS.top, parts, s, front.ox, fy1 - H * s - gapV - bb.max[1] * s, "top");
      const right = place(VIEWS.right, parts, s, fx0 + W * s + gapH + bb.max[1] * s, front.oy, "right");
      const placed = {};
      for (const [name, pv, S] of [["front", front, F], ["top", top, T], ["right", right, R]]) {
        const r = renderView(svg, pv, { grid: opts.grid, units: model.units });
        placed[name] = Object.assign(pv, { ext: r.ext, visible: r.visible, reserves: { left: reserve(S.maxRow.left || 0), right: reserve(S.maxRow.right || 0), top: reserve(S.maxRow.top || 0), bottom: reserve(S.maxRow.bottom || 0) } });
        drawSpecs(svg, pv, r.ext, S.specs, model.units);
        drawOverlays(svg, pv, model, opts.variant);
        const [x0, y0] = pv.P(r.ext.u0, r.ext.v0), [x1, y1] = pv.P(r.ext.u1, r.ext.v1);
        const sides = name === "front" ? ["top", "right"] : name === "top" ? ["right", "top"] : ["top", "right"];
        if (opts.balloons !== false) drawBalloons(svg, pv, r.visible, parts, sides, { x0, y0, x1, y1 });
        const rowsB = S.maxRow.bottom || 0;
        const ty = y1 + (rowsB ? ROW0 + ROW_STEP * (rowsB - 1) + 5 : 4) + 6;
        if (!svg.plain) viewTitle(svg, (x0 + x1) / 2, ty, pv.view.title + (opts.variantTitle ? ` · ${opts.variantTitle}` : ""), `SCALE ${sc.label}`);
      }
      drawCuttingPlanes(svg, placed, model);
      // isometric inset above the right view if it fits at full or half scale
      const isoW = (W + D) * COS30, isoH = (W + D) * SIN30 + H;
      const freeX0 = right.P(right.ext.u0, 0)[0] - 4, freeX1 = ar.x1, freeY0 = ar.y0, freeY1 = top.P(0, top.ext.v1)[1] + 6;
      for (const f of [1, 0.5]) {
        const si = s * f;
        if (isoW * si + 8 <= freeX1 - freeX0 && isoH * si + 14 <= freeY1 - freeY0) {
          const iso = VIEWS.iso;
          const e = extentOfBoxes(parts, iso);
          const ox = (freeX0 + freeX1) / 2 - ((e.u0 + e.u1) / 2) * si;
          const oy = freeY0 + 8 - e.v0 * si;
          const pv = place(iso, parts, si, ox, oy, "iso");
          renderView(svg, pv, { hiddenLines: false });
          if (!svg.plain) viewTitle(svg, (freeX0 + freeX1) / 2, oy + e.v1 * si + 7, "ISOMETRIC", f === 1 ? `SCALE ${sc.label}` : "HALF SCALE · REFERENCE ONLY");
          break;
        }
      }
      return { scale: s, scaleLabel: sc.label, notes: opts.notes };
    },
  };
}

function isoDims(svg, pv, bb, units) {
  const [W, D, H] = boxDims(bb);
  const { min, max } = bb;
  const P = pv.P3;
  // width along the front-bottom edge, extension lines down (−z)
  drawDim(svg, P([min[0], max[1], min[2]]), P([max[0], max[1], min[2]]), [0, 1], 10, fmt(W, units), { "data-view": pv.name, "data-axis": "x", "data-row": 1, "data-from": r3(min[0]), "data-to": r3(max[0]), "data-value": r3(W), "data-overall": "1" });
  // depth along the right-bottom edge
  drawDim(svg, P([max[0], min[1], min[2]]), P([max[0], max[1], min[2]]), [0, 1], 10, fmt(D, units), { "data-view": pv.name, "data-axis": "y", "data-row": 1, "data-from": r3(min[1]), "data-to": r3(max[1]), "data-value": r3(D), "data-overall": "1" });
  // height on the front-right vertical edge, extension along +x (down-right on paper)
  drawDim(svg, P([max[0], max[1], min[2]]), P([max[0], max[1], max[2]]), [COS30, SIN30], 10, fmt(H, units), { "data-view": pv.name, "data-axis": "z", "data-row": 1, "data-from": r3(min[2]), "data-to": r3(max[2]), "data-value": r3(H), "data-overall": "1" });
}

function isoSheet(model, parts, opts) {
  return {
    title: opts.title || "ISOMETRIC",
    draw(svg) {
      const ar = area(svg);
      const iso = VIEWS[opts.viewName || "iso"];
      const e = extentOfBoxes(parts, iso);
      const pad = opts.dims === false ? 12 : 26;
      const sc = pickScale(model.units, (s) => (e.u1 - e.u0) * s + 2 * pad, (s) => (e.v1 - e.v0) * s + 2 * pad, ar.x1 - ar.x0, ar.y1 - ar.y0, opts.scale);
      const s = sc.s;
      const ox = (ar.x0 + ar.x1) / 2 - ((e.u0 + e.u1) / 2) * s - (opts.dims === false ? 0 : 6);
      const oy = (ar.y0 + ar.y1) / 2 - ((e.v0 + e.v1) / 2) * s - 4;
      const pv = place(iso, parts, s, ox, oy, opts.viewName || "iso");
      if (opts.leaders) {
        // alignment leaders behind the parts: dash-dot from assembled centre to exploded centre
        for (const [from, to] of opts.leaders) {
          const A = pv.P3(from), B = pv.P3(to);
          svg.line(A[0], A[1], B[0], B[1], { class: "align", stroke: HID_INK, "stroke-width": LW.center, "stroke-dasharray": "5 1.2 1.2 1.2" });
        }
      }
      const r = renderView(svg, pv, { hiddenLines: false });
      if (opts.dims !== false) isoDims(svg, pv, assemblyBox(parts), model.units);
      const [x0, y0] = pv.P(r.ext.u0, r.ext.v0), [x1, y1] = pv.P(r.ext.u1, r.ext.v1);
      if (opts.balloons !== false) drawBalloons(svg, pv, r.visible, parts, ["left", "top", "right"], { x0, y0, x1, y1 }, 12);
      if (!svg.plain) viewTitle(svg, (x0 + x1) / 2, y1 + (opts.dims === false ? 8 : 22), opts.viewTitle || "ISOMETRIC · 30°", `SCALE ${sc.label}`);
      return { scale: s, scaleLabel: sc.label, notes: opts.notes };
    },
  };
}

function clipParts(parts, axis, at, keepSign) {
  // keepSign +1 keeps coordinate >= at; −1 keeps <= at
  const out = [];
  for (const p of parts) {
    const b = box([...p.box.min], [...p.box.max]);
    if (keepSign > 0) b.min[axis] = Math.max(b.min[axis], at); else b.max[axis] = Math.min(b.max[axis], at);
    if (b.max[axis] - b.min[axis] <= EPS) continue;
    const cuts = p.cuts.map((c) => boxIntersect(c, b)).filter(Boolean);
    out.push({ ...p, box: b, cuts });
  }
  return out;
}

function sectionSheet(model, parts, sec, opts) {
  const viewName = sec.axis === 1 ? (sec.look < 0 ? "front" : "back") : sec.axis === 0 ? (sec.look < 0 ? "right" : "left") : (sec.look < 0 ? "top" : "bottom");
  const keepSign = sec.look < 0 ? -1 : 1; // viewer on the + side keeps material at coordinate <= at
  const clipped = clipParts(parts, sec.axis, sec.at, keepSign);
  const view = VIEWS[viewName];
  const title = `SECTION ${sec.name}-${sec.name}`;
  return {
    title: sec.title ? `${title} · ${sec.title}` : title,
    draw(svg) {
      const ar = area(svg);
      const e = extentOfBoxes(clipped, view);
      const secModel = { ...model, dims: sec.dims.map((d) => ({ ...d, view: viewName })) };
      const S = specsFor(viewName, view, secModel, clipped, true);
      const resL = reserve(S.maxRow.left || 0), resB = reserve(S.maxRow.bottom || 0, true), resR = reserve(S.maxRow.right || 0), resT = reserve(S.maxRow.top || 0);
      const sc = pickScale(model.units, (s) => (e.u1 - e.u0) * s + resL + resR + 10, (s) => (e.v1 - e.v0) * s + resB + resT + 10, ar.x1 - ar.x0, ar.y1 - ar.y0, opts.scale);
      const s = sc.s;
      const ox = ar.x0 + resL + ((ar.x1 - ar.x0) - ((e.u1 - e.u0) * s + resL + resR + 10)) / 2 - e.u0 * s + 5;
      const oy = ar.y0 + resT + ((ar.y1 - ar.y0) - ((e.v1 - e.v0) * s + resB + resT + 10)) / 2 - e.v0 * s + 5;
      const pv = place(view, clipped, s, ox, oy, viewName);
      const r = renderView(svg, pv, { section: { axis: sec.axis, at: sec.at, viewerSign: -keepSign }, hiddenLines: false, units: model.units });
      drawSpecs(svg, pv, r.ext, S.specs, model.units);
      const [x0, y0] = pv.P(r.ext.u0, r.ext.v0), [x1, y1] = pv.P(r.ext.u1, r.ext.v1);
      drawBalloons(svg, pv, r.visible, clipped, ["top", "right"], { x0, y0, x1, y1 });
      const rowsB = S.maxRow.bottom || 0;
      if (!svg.plain) viewTitle(svg, (x0 + x1) / 2, y1 + (rowsB ? ROW0 + ROW_STEP * (rowsB - 1) + 5 : 4) + 6, title, `SCALE ${sc.label} · cut at ${AXIS_NAME[sec.axis]} = ${fmt(sec.at, model.units)}, looking ${sec.look < 0 ? "−" : "+"}${AXIS_NAME[sec.axis]}`);
      return { scale: s, scaleLabel: sc.label, notes: [...(opts.notes || []), "HATCHED AREAS ARE MATERIAL CUT BY THE SECTION PLANE"] };
    },
  };
}

function explodedParts(model, parts) {
  const bb = assemblyBox(parts);
  const maxDim = Math.max(...boxDims(bb));
  const vec = new Map();
  for (const p of parts) vec.set(p.id, [0, 0, 0]);
  const explicit = Object.keys(model.explode).length > 0;
  if (explicit) {
    for (const [id, v] of Object.entries(model.explode)) if (vec.has(id)) vec.set(id, v.map(num));
  } else {
    for (const j of model.joints) {
      if (!j.male) continue;
      const v = vec.get(j.male.id);
      if (!v) continue;
      const d = Math.max(j.depth * 4, maxDim * 0.12);
      v[j.entry] += j.entrySign * d * 0.5;
      const fv = vec.get(j.female.id);
      if (fv) fv[j.entry] -= j.entrySign * d * 0.5;
    }
  }
  const leaders = [];
  const out = parts.map((p) => {
    const v = vec.get(p.id);
    if (!v || v.every((c) => c === 0)) return p;
    const c0 = boxCenter(p.box);
    const np = { ...p, box: boxTranslate(p.box, v), cuts: p.cuts.map((c) => boxTranslate(c, v)) };
    leaders.push([c0, boxCenter(np.box)]);
    return np;
  });
  return { parts: out, leaders };
}

// ───────────────────────────── part cards ─────────────────────────────
function orientPart(part) {
  const dims = boxDims(part.box);
  const order = [0, 1, 2].sort((a, b) => dims[b] - dims[a]); // L, W, T world axes
  const [La, Wa, Ta] = order;
  // choose the T sign so the face with more cut volume faces +y (visible in the face view)
  let plus = 0, minus = 0;
  for (const c of part.cuts) {
    const v = boxVolume(c);
    if (nearly(c.max[Ta], part.box.max[Ta], 1e-6)) plus += v;
    if (nearly(c.min[Ta], part.box.min[Ta], 1e-6)) minus += v;
  }
  const flipT = minus > plus;
  const map = (p) => {
    const x = p[La] - part.box.min[La];
    const z = p[Wa] - part.box.min[Wa];
    const y = flipT ? part.box.max[Ta] - p[Ta] : p[Ta] - part.box.min[Ta];
    return [x, y, z];
  };
  const mapBox = (b) => {
    const a = map(b.min), c = map(b.max);
    return box([Math.min(a[0], c[0]), Math.min(a[1], c[1]), Math.min(a[2], c[2])], [Math.max(a[0], c[0]), Math.max(a[1], c[1]), Math.max(a[2], c[2])]);
  };
  const local = { ...part, box: mapBox(part.box), cuts: part.cuts.map(mapBox), cutInfo: part.cutInfo.map((ci) => ({ ...ci, box: mapBox(ci.box) })) };
  return { local, L: dims[La], W: dims[Wa], T: dims[Ta] };
}

function partCard(svg, part, cx0, cy0, cw, ch, model, opts) {
  const { local, L, W, T } = orientPart(part);
  const units = model.units;
  if (!svg.plain) {
    svg.rect(cx0, cy0, cw, ch, { fill: "none", stroke: INK, "stroke-width": LW.dim, class: "card" });
    svg.text(cx0 + 3, cy0 + 5.2, `${part.item}  ${part.name.toUpperCase()}`, { "font-size": TXT.label, "font-weight": "700" });
    const qty = model.parts.filter((p) => p.name === part.name).reduce((a, p) => a + p.qty, 0);
    svg.text(cx0 + cw - 3, cy0 + 5.2, `QTY ${qty}${part.material ? " · " + part.material.toUpperCase() : ""}`, { "font-size": 2.6, "text-anchor": "end", fill: "#333" });
    svg.line(cx0, cy0 + 8, cx0 + cw, cy0 + 8, { stroke: INK, "stroke-width": LW.dim });
  }
  // dims: chains from cuts in the face view (x) and left (z); depth dims in the top view (left) and right view (bottom)
  const faceSpecs = [], topSpecs = [], rightSpecs = [];
  const chain = (axis, full, cuts, side, viewSpecs) => {
    const segs = cuts.filter((c) => !(nearly(c.min[axis], 0, 1e-6) && nearly(c.max[axis], full, 1e-6))).map((c) => [c.min[axis], c.max[axis]]).sort((a, b) => a[0] - b[0]);
    if (!segs.length) { viewSpecs.push({ axis, from: 0, to: full, row: 1, side, overall: true }); return; }
    let cur = 0;
    const merged = [];
    for (const s of segs) { if (merged.length && s[0] <= merged[merged.length - 1][1] + 1e-6) merged[merged.length - 1][1] = Math.max(merged[merged.length - 1][1], s[1]); else merged.push([...s]); }
    for (const [a, b] of merged) {
      if (a - cur > 1e-6) viewSpecs.push({ axis, from: cur, to: a, row: 1, side });
      viewSpecs.push({ axis, from: a, to: b, row: 1, side });
      cur = b;
    }
    if (full - cur > 1e-6) viewSpecs.push({ axis, from: cur, to: full, row: 1, side });
    viewSpecs.push({ axis, from: 0, to: full, row: 2, side, overall: true });
  };
  chain(0, L, local.cuts, "bottom", faceSpecs);
  chain(2, W, local.cuts, "left", faceSpecs);
  // depth (y) of cuts: shown in the top view for cuts running through z, else in the right view
  const depthCuts = local.cuts.filter((c) => c.max[1] - c.min[1] < T - 1e-6);
  const seen = new Set();
  for (const c of depthCuts) {
    const throughZ = nearly(c.min[2], 0, 1e-6) && nearly(c.max[2], W, 1e-6);
    const key = `${throughZ}:${r3(c.min[1])}-${r3(c.max[1])}`;
    if (seen.has(key)) continue;
    seen.add(key);
    (throughZ ? topSpecs : rightSpecs).push({ axis: 1, from: c.min[1], to: c.max[1], row: 1, side: throughZ ? "left" : "bottom" });
  }
  topSpecs.push({ axis: 1, from: 0, to: T, row: topSpecs.length ? 2 : 1, side: "left", overall: true });
  rightSpecs.push({ axis: 1, from: 0, to: T, row: rightSpecs.length ? 2 : 1, side: "bottom", overall: true });
  const rowsL = 2, rowsB = 2, rowsTopL = topSpecs.length > 1 ? 2 : 1;
  const resL = reserve(Math.max(rowsL, rowsTopL)), resB = reserve(rowsB, false) + 2;
  const gapH = 20, gapV = 9;
  const innerX0 = cx0 + 3 + resL, innerX1 = cx0 + cw - 3 - 10;
  const innerY0 = cy0 + 17, innerY1 = cy0 + ch - 3 - resB;
  const needW = (s) => L * s + gapH + T * s + reserve(2);
  const needH = (s) => T * s + gapV + W * s;
  const sc = pickScale(units, needW, needH, innerX1 - innerX0, innerY1 - innerY0, opts.scale);
  const s = sc.s;
  const fx0 = innerX0 + Math.max(0, (innerX1 - innerX0 - needW(s)) / 2);
  const fy1 = innerY1 - Math.max(0, (innerY1 - innerY0 - needH(s)) / 2);
  const face = place(VIEWS.front, [local], s, fx0, fy1, "front");
  const topv = place(VIEWS.top, [local], s, fx0, fy1 - W * s - gapV - T * s, "top");
  const rightv = place(VIEWS.right, [local], s, fx0 + L * s + gapH + T * s, fy1, "right");
  const rf = renderView(svg, face, { grid: opts.grid, units });
  const rt = renderView(svg, topv, { units });
  const rr = renderView(svg, rightv, { units });
  drawSpecs(svg, face, rf.ext, faceSpecs, units);
  drawSpecs(svg, topv, rt.ext, topSpecs, units);
  drawSpecs(svg, rightv, rr.ext, rightSpecs, units);
  if (!svg.plain) {
    const [x0] = face.P(rf.ext.u0, 0), [x1, y1] = face.P(rf.ext.u1, rf.ext.v1);
    svg.text(cx0 + 3, cy0 + ch - 3, `SCALE ${sc.label} · L ${fmt(L, units)} × W ${fmt(W, units)} × T ${fmt(T, units)}${part.grain ? " · GRAIN ALONG L" : ""}`, { "font-size": 2.4, fill: "#333" });
    // name the cuts
    let ny = cy0 + 11;
    for (const ci of local.cutInfo) {
      const d = boxDims(ci.box);
      svg.text(cx0 + cw - 3, ny + 2.4, `${ci.kind.toUpperCase()}${ci.name && ci.name !== ci.kind ? " · " + ci.name : ""}: ${fmt(d[0], units)} × ${fmt(d[2], units)} × ${fmt(d[1], units)} deep`, { "font-size": 2.2, "text-anchor": "end", fill: "#333" });
      ny += 3.2;
      if (ny > cy0 + 24) break;
    }
  }
  return sc;
}

function partsSheets(model, parts, opts) {
  const unique = [];
  const seen = new Set();
  for (const p of parts) { if (seen.has(p.name)) continue; seen.add(p.name); unique.push(p); }
  const perSheet = 4;
  const sheets = [];
  for (let i = 0; i < unique.length; i += perSheet) {
    const chunk = unique.slice(i, i + perSheet);
    sheets.push({
      title: `PART DRAWINGS ${Math.floor(i / perSheet) + 1}`,
      draw(svg) {
        const ar = area(svg);
        const cols = 2, rows = 2, g = 4;
        const cw = (ar.x1 - ar.x0 - g) / cols, ch = (ar.y1 - ar.y0 - g) / rows;
        let scales = [];
        chunk.forEach((part, k) => {
          const cx = ar.x0 + (k % cols) * (cw + g), cy = ar.y0 + Math.floor(k / cols) * (ch + g);
          scales.push(partCard(svg, part, cx, cy, cw, ch, model, opts).label);
        });
        return { scale: null, scaleLabel: "AS NOTED", notes: ["EACH CARD STATES ITS OWN SCALE · DIMENSIONS ARE FINISHED SIZES"] };
      },
    });
  }
  // parts list sheet
  sheets.push({
    title: "PARTS LIST",
    draw(svg) {
      const ar = area(svg);
      const cols = [ar.x0, ar.x0 + 12, ar.x0 + 24, ar.x0 + 90, ar.x0 + 150, ar.x1];
      const heads = ["ITEM", "QTY", "PART", "FINISHED L × W × T", "MATERIAL"];
      let y = ar.y0 + 6;
      svg.open("g", { class: "parts-list" });
      heads.forEach((h, i) => svg.text(cols[i] + 2, y, h, { "font-size": 2.8, "font-weight": "700" }));
      y += 2.5;
      svg.line(ar.x0, y, ar.x1, y, { stroke: INK, "stroke-width": LW.dim });
      for (const row of partsTable(model)) {
        y += 6;
        if (y > ar.y1) break;
        const cells = [String(row.item), String(row.qty), row.name, `${fmt(row.size[0], model.units)} × ${fmt(row.size[1], model.units)} × ${fmt(row.size[2], model.units)}`, row.material];
        cells.forEach((c, i) => svg.text(cols[i] + 2, y, c, { "font-size": 2.8 }));
        svg.line(ar.x0, y + 2, ar.x1, y + 2, { stroke: "#bbb", "stroke-width": 0.15 });
      }
      svg.close("g");
      return { scale: null, scaleLabel: "—", notes: ["ITEM NUMBERS MATCH THE BALLOONS ON EVERY SHEET"] };
    },
  });
  return sheets;
}

export function partsTable(model) {
  const rows = new Map();
  for (const p of model.parts) {
    const key = p.name;
    if (!rows.has(key)) rows.set(key, { item: p.item, name: p.name, qty: 0, size: [...p.size], material: p.material, cutlist: p.cutlist });
    rows.get(key).qty += p.qty;
  }
  return [...rows.values()].sort((a, b) => a.item - b.item);
}

// ───────────────────────────── joint teaching sheets ─────────────────────────────
function jointSignature(j) {
  return `${j.type}|${j.female.name}|${j.male.name}|${r3(j.width)}|${r3(j.depth)}|${j.tenon ? r3(j.tenonThickness) : ""}`;
}

function jointSheets(model, parts, opts) {
  const groups = new Map();
  for (const j of model.joints) {
    if (!j.male || j.detail === false) continue;
    const key = jointSignature(j);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(j);
  }
  const sheets = [];
  let idx = 0;
  for (const list of groups.values()) {
    const j = list[0];
    idx++;
    const n = idx;
    const units = model.units;
    const typeName = j.tenon ? "STUB TENON" : j.type.toUpperCase();
    const dimsLabel = j.tenon
      ? `${fmt(j.tenonThickness, units)} THICK × ${fmt(j.depth, units)} LONG TENON`
      : `${fmt(j.width, units)} WIDE × ${fmt(j.depth, units)} DEEP`;
    const occurs = j.occurs || `${list.length} LOCATION${list.length === 1 ? "" : "S"}`;
    sheets.push({
      title: `JOINT ${n} · ${typeName} · ${j.female.name.toUpperCase()} ↔ ${j.male.name.toUpperCase()}`,
      draw(svg) {
        const ar = area(svg);
        const minPad = units === "in" ? 1.25 : 32;
        const region = box([...j.overlap.min], [...j.overlap.max]);
        for (let a = 0; a < 3; a++) {
          const m = a === j.axis ? Math.max(j.depth * 3, minPad) : a === j.widthAxis ? Math.max(j.width * 3, minPad) : Math.max(j.width * 2.5, minPad);
          region.min[a] -= m; region.max[a] += m;
        }
        const clipTo = (p) => {
          const b = boxIntersect(p.box, region);
          const planes = [];
          for (let a = 0; a < 3; a++) {
            if (b.min[a] > p.box.min[a] + EPS) planes.push({ axis: a, at: b.min[a] });
            if (b.max[a] < p.box.max[a] - EPS) planes.push({ axis: a, at: b.max[a] });
          }
          return { part: { ...p, box: b, cuts: p.cuts.map((c) => boxIntersect(c, b)).filter(Boolean) }, planes };
        };
        const F = clipTo(j.female), M = clipTo(j.male);
        const breakPlanes = [...F.planes, ...M.planes];
        const offset = j.depth * 2.5 + j.width * 1.5;
        const vec = [0, 0, 0]; vec[j.entry] = j.entrySign * offset;
        const Msep = { ...M.part, box: boxTranslate(M.part.box, vec), cuts: M.part.cuts.map((c) => boxTranslate(c, vec)) };
        const sepPlanes = [...F.planes, ...M.planes.map((pl) => (pl.axis === j.entry ? { axis: pl.axis, at: pl.at + vec[j.entry] } : pl))];
        const iso = VIEWS.iso;
        const eS = extentOfBoxes([F.part, Msep], iso), eA = extentOfBoxes([F.part, M.part], iso);
        // section through the joint centre along the length axis
        const la = j.lengthAxis;
        const centre = (j.overlap.min[la] + j.overlap.max[la]) / 2;
        const secParts = clipParts([F.part, M.part], la, centre, -1);
        const secViewName = la === 1 ? "front" : la === 0 ? "right" : "top";
        const secView = VIEWS[secViewName];
        const eC = extentOfBoxes(secParts, secView);
        const specs = [];
        const sideFor = (axis) => (axis === secView.uAxis ? "bottom" : "left");
        const fb = F.part.box;
        const remMin = j.overlap.min[j.axis] - fb.min[j.axis], remMax = fb.max[j.axis] - j.overlap.max[j.axis];
        const sd = sideFor(j.axis);
        if (remMin > EPS) specs.push({ axis: j.axis, from: fb.min[j.axis], to: j.overlap.min[j.axis], row: 1, side: sd });
        specs.push({ axis: j.axis, from: j.overlap.min[j.axis], to: j.overlap.max[j.axis], row: 1, side: sd });
        if (remMax > EPS) specs.push({ axis: j.axis, from: j.overlap.max[j.axis], to: fb.max[j.axis], row: 1, side: sd });
        if (remMin > EPS || remMax > EPS) specs.push({ axis: j.axis, from: fb.min[j.axis], to: fb.max[j.axis], row: 2, side: sd, overall: true });
        const sw = sideFor(j.widthAxis);
        if (j.tenon) {
          const t = j.tenon;
          const mb = M.part.box;
          specs.push({ axis: j.widthAxis, from: mb.min[j.widthAxis], to: t.min[j.widthAxis], row: 1, side: sw });
          specs.push({ axis: j.widthAxis, from: t.min[j.widthAxis], to: t.max[j.widthAxis], row: 1, side: sw });
          specs.push({ axis: j.widthAxis, from: t.max[j.widthAxis], to: mb.max[j.widthAxis], row: 1, side: sw });
          specs.push({ axis: j.widthAxis, from: mb.min[j.widthAxis], to: mb.max[j.widthAxis], row: 2, side: sw, overall: true });
        } else {
          specs.push({ axis: j.widthAxis, from: j.overlap.min[j.widthAxis], to: j.overlap.max[j.widthAxis], row: 1, side: sw });
        }
        const rowsB = Math.max(0, ...specs.filter((s) => s.side === "bottom").map((s) => s.row));
        const rowsL = Math.max(0, ...specs.filter((s) => s.side === "left").map((s) => s.row));
        const resL = reserve(rowsL), resB = reserve(rowsB, true);
        const gap = 16;
        const labelPad = 16;
        const needW = (s) => (eS.u1 - eS.u0) * s + gap + (eA.u1 - eA.u0) * s + gap + resL + (eC.u1 - eC.u0) * s + 6;
        const needH = (s) => Math.max((eS.v1 - eS.v0) * s, (eA.v1 - eA.v0) * s, (eC.v1 - eC.v0) * s + resB) + labelPad + 14;
        const sc = pickScale(units, needW, needH, ar.x1 - ar.x0, ar.y1 - ar.y0 - 10, opts.scale);
        const s = sc.s;
        const top = ar.y0 + 12 + labelPad;
        let x = ar.x0 + Math.max(0, (ar.x1 - ar.x0 - needW(s)) / 2);
        const rowH = needH(s) - labelPad - 14;
        const placeIso = (partsList, e, name) => {
          const pv = place(iso, partsList, s, x - e.u0 * s, top + (rowH - (e.v1 - e.v0) * s) / 2 - e.v0 * s, name);
          x += (e.u1 - e.u0) * s + gap;
          return pv;
        };
        const pvS = placeIso([F.part, Msep], eS, "iso-separated");
        const rS = renderView(svg, pvS, { hiddenLines: false, breakPlanes: sepPlanes });
        const pvA = placeIso([F.part, M.part], eA, "iso-assembled");
        const rA = renderView(svg, pvA, { hiddenLines: false, breakPlanes });
        x += resL;
        const pvC = place(secView, secParts, s, x - eC.u0 * s, top + (rowH - (eC.v1 - eC.v0) * s - resB) / 2 - eC.v0 * s, secViewName);
        const rC = renderView(svg, pvC, { hiddenLines: false, section: { axis: la, at: centre, viewerSign: 1 }, breakPlanes });
        drawSpecs(svg, pvC, rC.ext, specs, units);
        if (!svg.plain) {
          const mc = boxCenter(Msep.box);
          const faceC = [...mc]; faceC[j.entry] = j.entrySign > 0 ? Msep.box.min[j.entry] : Msep.box.max[j.entry];
          const tipP = [...faceC]; tipP[j.entry] -= j.entrySign * offset * 0.7;
          const A = pvS.P3(faceC), B = pvS.P3(tipP);
          const d = Math.hypot(B[0] - A[0], B[1] - A[1]) || 1;
          svg.line(A[0], A[1], B[0], B[1], { stroke: DIM_INK, "stroke-width": 0.4, class: "leader" });
          arrowHead(svg, B, [(A[0] - B[0]) / d, (A[1] - B[1]) / d]);
          const labelParts = (pv, vis, list, roles) => {
            list.forEach((p, i) => {
              const a = anchorFor(pv, vis, p);
              if (!a) return;
              const e = extentOfBoxes(list, pv.view);
              const [x0, y0] = pv.P(e.u0, e.v0), [x1] = pv.P(e.u1, e.v1);
              const lx = i === 0 ? x0 + 2 : x1 - 2;
              const ly = y0 - (i === 0 ? 12 : 7);
              const txt = `${p.name.toUpperCase()} (${roles[i]})`;
              svg.line(lx, ly + 1.2, a[0], a[1], { stroke: DIM_INK, "stroke-width": LW.leader, class: "leader" });
              svg.circle(a[0], a[1], 0.6, { fill: DIM_INK });
              svg.text(lx, ly, txt, { "font-size": 2.8, "text-anchor": i === 0 ? "start" : "end" });
            });
          };
          labelParts(pvS, rS.visible, [F.part, Msep], ["FEMALE", "MALE"]);
          labelParts(pvA, rA.visible, [F.part, M.part], ["FEMALE", "MALE"]);
          const cap = (pv, r, t, sub) => {
            const [x0] = pv.P(r.ext.u0, 0), [x1] = pv.P(r.ext.u1, r.ext.v1);
            viewTitle(svg, (x0 + x1) / 2, top + rowH + 9, t, sub);
          };
          cap(pvS, rS, "SEPARATED", "along the assembly axis");
          cap(pvA, rA, "ASSEMBLED", "");
          cap(pvC, rC, "SECTION", `through the joint · SCALE ${sc.label}`);
          svg.text(ar.x0, ar.y0 + 6, `${typeName} · ${dimsLabel}`, { "font-size": TXT.title, "font-weight": "700" });
          svg.text(ar.x0, ar.y0 + 11, `${j.female.name.toUpperCase()} is female (receives the cut) · ${j.male.name.toUpperCase()} is male · OCCURS: ${occurs}${j.note ? " · " + j.note : ""}`, { "font-size": 2.6, fill: "#333" });
          svg.text(ar.x1, ar.y0 + 6, "ENDS CLIPPED FOR CLARITY — THIN OUTLINES ARE BREAKS, NOT EDGES", { "font-size": 2.2, "text-anchor": "end", fill: "#555" });
        }
        return { scale: s, scaleLabel: sc.label, notes: opts.notes };
      },
    });
  }
  return sheets;
}

// ───────────────────────────── render driver ─────────────────────────────
const slugify = (s) => String(s).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "drawing";

export function buildSheets(model, opts = {}) {
  const parts = variantParts(model, opts.variant);
  const only = opts.only ? new Set(String(opts.only).split(",")) : null;
  const want = (k) => !only || only.has(k);
  const sheets = [];
  const vt = opts.variant ? opts.variant.toUpperCase().replace(/-/g, " ") : "";
  if (want("ortho")) sheets.push({ kind: "ortho", ...orthoSheet(model, parts, { ...opts, variantTitle: vt, title: `ORTHOGRAPHIC SET${vt ? " · " + vt : ""}` }) });
  if (want("iso")) sheets.push({ kind: "iso", ...isoSheet(model, parts, { ...opts, title: `ISOMETRIC${vt ? " · " + vt : ""}` }) });
  if (want("section")) model.sections.forEach((sec) => sheets.push({ kind: `section-${slugify(sec.name)}`, ...sectionSheet(model, parts, sec, opts) }));
  if (want("exploded")) {
    const ex = explodedParts(model, parts);
    sheets.push({ kind: "exploded", ...isoSheet(model, ex.parts, { ...opts, title: "EXPLODED ASSEMBLY", viewTitle: "EXPLODED · 30° ISOMETRIC", dims: false, leaders: ex.leaders, notes: ["PARTS MOVED ALONG THEIR TRUE ASSEMBLY AXES · CHAIN LINES RECONNECT ASSEMBLED POSITIONS"] }) });
  }
  if (want("joints")) jointSheets(model, parts, opts).forEach((sh, i) => sheets.push({ kind: `joint-${i + 1}`, ...sh }));
  if (want("parts")) partsSheets(model, parts, opts).forEach((sh, i) => sheets.push({ kind: sh.title === "PARTS LIST" ? "parts-list" : `parts-${i + 1}`, ...sh }));
  return sheets;
}

export function renderSheets(model, opts = {}) {
  const sheetSize = SHEETS[opts.sheet || model.sheet] || SHEETS.letter;
  const slug = slugify(model.title);
  const sheets = buildSheets(model, opts);
  const out = [];
  sheets.forEach((sh, i) => {
    const svg = new Svg(sheetSize, `${slug}-${sh.kind}`, !!opts.plain);
    const info = sh.draw(svg) || {};
    sheetFrame(svg, model, { title: sh.title, n: i + 1, m: sheets.length, scale: info.scale, scaleLabel: info.scaleLabel || "—", notes: [...(info.notes || []), ...(model.notes || [])] });
    out.push({ name: `${slug}-${sh.kind}${opts.variant ? "-" + slugify(opts.variant) : ""}${opts.plain ? "-plain" : ""}.svg`, kind: sh.kind, title: sh.title, svg: svg.toString(), scale: info.scale });
  });
  return out;
}

function writeOutputs(model, files, opts) {
  fs.mkdirSync(opts.out, { recursive: true });
  const written = [];
  for (const f of files) {
    const p = path.join(opts.out, f.name);
    fs.writeFileSync(p, f.svg);
    written.push(p);
    if (opts.png) {
      const png = p.replace(/\.svg$/, ".png");
      try {
        const sheet = SHEETS[opts.sheet || model.sheet] || SHEETS.letter;
        const px = Math.round((sheet.w / MM_PER_IN) * (Number(opts.dpi) || 300));
        execFileSync("rsvg-convert", ["-w", String(px), p, "-o", png], { stdio: "pipe" });
        written.push(png);
      } catch (e) {
        console.error(`png: rsvg-convert unavailable or failed for ${p}: ${String(e.message).split("\n")[0]}`);
      }
    }
  }
  if (opts.dxf) {
    for (const v of ["front", "top", "right"]) {
      const p = path.join(opts.out, `${slugify(model.title)}-${v}.dxf`);
      fs.writeFileSync(p, dxfFor(model, variantParts(model, opts.variant), v));
      written.push(p);
    }
  }
  return written;
}

// ───────────────────────────── DXF ─────────────────────────────
export function dxfFor(model, parts, viewName) {
  const view = VIEWS[viewName];
  if (!view) throw new Error(`unknown view ${viewName}`);
  const { visible, hidden } = projectEdges(parts, view, { hidden: view.kind === "ortho" });
  const L = [];
  const push = (...v) => L.push(...v);
  push("0", "SECTION", "2", "HEADER", "9", "$ACADVER", "1", "AC1009", "9", "$INSUNITS", "70", model.units === "mm" ? "4" : "1", "0", "ENDSEC");
  push("0", "SECTION", "2", "TABLES", "0", "TABLE", "2", "LTYPE", "70", "2",
    "0", "LTYPE", "2", "CONTINUOUS", "70", "0", "3", "Solid line", "72", "65", "73", "0", "40", "0.0",
    "0", "LTYPE", "2", "HIDDEN", "70", "0", "3", "Hidden __ __ __", "72", "65", "73", "2", "40", "0.375", "49", "0.25", "49", "-0.125",
    "0", "ENDTAB", "0", "TABLE", "2", "LAYER", "70", "2",
    "0", "LAYER", "2", "VISIBLE", "70", "0", "62", "7", "6", "CONTINUOUS",
    "0", "LAYER", "2", "HIDDEN", "70", "0", "62", "8", "6", "HIDDEN",
    "0", "ENDTAB", "0", "ENDSEC");
  push("0", "SECTION", "2", "ENTITIES");
  const line = (s, layer) => push("0", "LINE", "8", layer, "10", r3(s.u0), "20", r3(-s.v0), "30", "0", "11", r3(s.u1), "21", r3(-s.v1), "31", "0");
  for (const s of visible) line(s, "VISIBLE");
  for (const s of hidden) line(s, "HIDDEN");
  push("0", "ENDSEC", "0", "EOF");
  return L.join("\n") + "\n";
}

// ───────────────────────────── SVG parsing for checks ─────────────────────────────
function parseSvg(text) {
  const els = [];
  const stack = [];
  const re = /<\/?([A-Za-z][\w:-]*)([^>]*?)(\/?)>/g;
  const attrRe = /([\w:-]+)\s*=\s*"([^"]*)"/g;
  const parseAttrs = (s) => { const o = {}; let a; attrRe.lastIndex = 0; while ((a = attrRe.exec(s))) o[a[1]] = a[2]; return o; };
  const translateOf = (t) => { const r = /translate\(\s*(-?[\d.]+)[ ,]*(-?[\d.]+)?\s*\)/.exec(t || ""); return r ? [Number(r[1]), Number(r[2] || 0)] : [0, 0]; };
  let m;
  while ((m = re.exec(text))) {
    const [full, tag, rest, selfClose] = m;
    if (full.startsWith("</")) { if (stack.length && stack[stack.length - 1].tag === tag) stack.pop(); continue; }
    const a = parseAttrs(rest);
    const [tx, ty] = translateOf(a.transform);
    const inhTx = stack.reduce((acc, s) => acc + s.tx, 0), inhTy = stack.reduce((acc, s) => acc + s.ty, 0);
    const el = { tag, attrs: a, tx: inhTx + tx, ty: inhTy + ty, groups: stack.slice(), index: m.index };
    if (tag === "text") {
      const close = text.indexOf("</text>", m.index);
      el.content = text.slice(m.index + full.length, close).replace(/<[^>]+>/g, "").replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').trim();
    }
    const inDefs = stack.some((s) => s.tag === "defs");
    if (tag !== "defs" && !inDefs) els.push(el);
    if (!selfClose && tag !== "text") stack.push({ tag, attrs: a, tx, ty });
    else if (tag === "text" && !selfClose) { /* text closes itself via </text> which is handled above */ stack.push({ tag, attrs: a, tx: 0, ty: 0 }); }
  }
  return els;
}

function segmentsOf(el) {
  const a = el.attrs, tx = el.tx, ty = el.ty;
  const n = (k) => Number(a[k] || 0);
  const segs = [];
  if (a.stroke === "none") return segs;
  if (el.tag === "line") segs.push([[n("x1") + tx, n("y1") + ty], [n("x2") + tx, n("y2") + ty]]);
  else if (el.tag === "polyline" || el.tag === "polygon") {
    const pts = (a.points || "").trim().split(/[\s,]+/).map(Number);
    const count = pts.length / 2;
    for (let i = 0; i < count - (el.tag === "polygon" ? 0 : 1); i++) {
      const j = (i + 1) % count;
      segs.push([[pts[2 * i] + tx, pts[2 * i + 1] + ty], [pts[2 * j] + tx, pts[2 * j + 1] + ty]]);
    }
  } else if (el.tag === "rect") {
    const x = n("x") + tx, y = n("y") + ty, w = n("width"), h = n("height");
    if (a.stroke && a.stroke !== "none") segs.push([[x, y], [x + w, y]], [[x + w, y], [x + w, y + h]], [[x + w, y + h], [x, y + h]], [[x, y + h], [x, y]]);
  } else if (el.tag === "circle") {
    const cx = n("cx") + tx, cy = n("cy") + ty, r = n("r");
    if (a.stroke && a.stroke !== "none") for (let i = 0; i < 12; i++) {
      const t0 = (i / 12) * 2 * Math.PI, t1 = ((i + 1) / 12) * 2 * Math.PI;
      segs.push([[cx + r * Math.cos(t0), cy + r * Math.sin(t0)], [cx + r * Math.cos(t1), cy + r * Math.sin(t1)]]);
    }
  } else if (el.tag === "path") {
    const d = a.d || "";
    const tok = d.match(/[MLHVZmlhvz]|-?\d*\.?\d+(?:e-?\d+)?/g) || [];
    let cmd = null, cur = [0, 0], start = [0, 0], i = 0;
    while (i < tok.length) {
      const t = tok[i++];
      if (/[A-Za-z]/.test(t)) { cmd = t; if (cmd === "Z" || cmd === "z") { segs.push([[...cur], [...start]]); cur = [...start]; } continue; }
      const v = Number(t);
      if (cmd === "M" || cmd === "L" || cmd === "m" || cmd === "l") {
        const w = Number(tok[i++]);
        const p = cmd === cmd.toLowerCase() ? [cur[0] + v, cur[1] + w] : [v, w];
        if (cmd === "M" || cmd === "m") { start = p; cmd = cmd === "M" ? "L" : "l"; } else segs.push([[...cur], p]);
        cur = p;
      } else if (cmd === "H" || cmd === "h") { const p = [cmd === "h" ? cur[0] + v : v, cur[1]]; segs.push([[...cur], p]); cur = p; }
      else if (cmd === "V" || cmd === "v") { const p = [cur[0], cmd === "v" ? cur[1] + v : v]; segs.push([[...cur], p]); cur = p; }
      else break; // curves are not audited
    }
    return segs.map(([p, q]) => [[p[0] + tx, p[1] + ty], [q[0] + tx, q[1] + ty]]);
  }
  return segs;
}

function textBox(el, unitToMm = 1) {
  const a = el.attrs;
  const size = Number(a["font-size"] || 3);
  const w = textWidth(el.content || "", size);
  const x = Number(a.x || 0) + el.tx, y = Number(a.y || 0) + el.ty;
  const anchor = a["text-anchor"] || "start";
  const x0 = anchor === "middle" ? x - w / 2 : anchor === "end" ? x - w : x;
  let bx = { x0, y0: y - size * 0.75, x1: x0 + w, y1: y + size * 0.25 };
  const rot = /rotate\(\s*(-?[\d.]+)(?:[ ,]+(-?[\d.]+)[ ,]+(-?[\d.]+))?\s*\)/.exec(a.transform || "");
  if (rot) {
    const ang = (Number(rot[1]) * Math.PI) / 180, cx = rot[2] != null ? Number(rot[2]) + el.tx : x, cy = rot[3] != null ? Number(rot[3]) + el.ty : y;
    const pts = [[bx.x0, bx.y0], [bx.x1, bx.y0], [bx.x1, bx.y1], [bx.x0, bx.y1]].map(([px, py]) => [cx + (px - cx) * Math.cos(ang) - (py - cy) * Math.sin(ang), cy + (px - cx) * Math.sin(ang) + (py - cy) * Math.cos(ang)]);
    bx = { x0: Math.min(...pts.map((p) => p[0])), y0: Math.min(...pts.map((p) => p[1])), x1: Math.max(...pts.map((p) => p[0])), y1: Math.max(...pts.map((p) => p[1])) };
  }
  return { ...bx, size, sizeMm: size * unitToMm, content: el.content, el };
}

function segHitsRect(p, q, r) {
  const inside = (pt) => pt[0] > r.x0 && pt[0] < r.x1 && pt[1] > r.y0 && pt[1] < r.y1;
  if (inside(p) || inside(q)) return true;
  const edges = [[[r.x0, r.y0], [r.x1, r.y0]], [[r.x1, r.y0], [r.x1, r.y1]], [[r.x1, r.y1], [r.x0, r.y1]], [[r.x0, r.y1], [r.x0, r.y0]]];
  return edges.some(([a, b]) => seg2dIntersectT(p, q, a, b) != null);
}
const rectsOverlap = (a, b) => a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1;

// ───────────────────────────── checks ─────────────────────────────
export function checkSvg(text, opts = {}) {
  const findings = [];
  const add = (check, status, detail, extra = {}) => findings.push({ check, status, detail, ...extra });
  const els = parseSvg(text);
  const root = els.find((e) => e.tag === "svg");
  const vb = (root?.attrs.viewBox || "").split(/[\s,]+/).map(Number);
  const isDraft = root?.attrs["data-draft"] === "1";
  let unitToMm = 1;
  if (opts.printWidthMm && vb.length === 4) unitToMm = opts.printWidthMm / vb[2];
  else if (!isDraft && vb.length === 4 && /mm$/.test(root?.attrs.width || "")) unitToMm = parseFloat(root.attrs.width) / vb[2];
  const minText = opts.minTextMm ?? 2.0, minDimText = opts.minDimTextMm ?? 2.5;
  const tol = opts.units === "mm" ? 0.05 : 1 / 128;
  const hasClass = (el, c) => (el.attrs.class || "").split(/\s+/).includes(c);
  const views = els.filter((e) => e.tag === "g" && hasClass(e, "view"));
  const strokeEls = els.filter((e) => ["line", "polyline", "polygon", "path", "rect", "circle"].includes(e.tag) && !hasClass(e, "face") && !hasClass(e, "cut-face") && !hasClass(e, "grid") && !hasClass(e, "extent") && e.attrs.stroke !== "none");
  const inGroup = (e, g) => e.groups.some((s) => s.attrs === g.attrs);

  // 1. projection audit
  const viewGroups = views.length ? views : [null];
  for (const g of viewGroups) {
    const inG = (e) => (g ? inGroup(e, g) : true);
    const kind = g?.attrs["data-kind"];
    const name = g ? g.attrs["data-view"] : "(whole file)";
    const isoClaimed = kind === "iso" || (!g && /isometric/i.test(text));
    if (!isoClaimed) continue;
    const axisLines = strokeEls.filter((e) => inG(e) && e.attrs["data-axis"] && (hasClass(e, "obj") || hasClass(e, "brk") || hasClass(e, "cut") || hasClass(e, "hid")));
    if (axisLines.length) {
      const expected = { x: 30, y: 150, z: 90 };
      const bad = [];
      const ratios = { x: [], y: [], z: [] };
      for (const e of axisLines) {
        const [[x0, y0], [x1, y1]] = segmentsOf(e)[0];
        let ang = (Math.atan2(y1 - y0, x1 - x0) * 180) / Math.PI;
        ang = ((ang % 180) + 180) % 180;
        const ax = e.attrs["data-axis"];
        const err = Math.min(Math.abs(ang - expected[ax]), Math.abs(ang - expected[ax] + 180), Math.abs(ang - expected[ax] - 180));
        const len = Math.hypot(x1 - x0, y1 - y0);
        if (len > 0.5 && err > 0.5) bad.push(`${ax}-axis edge at ${ang.toFixed(2)}° (expected ${expected[ax]}°)`);
        const ml = Number(e.attrs["data-len"] || 0);
        if (ml > 0 && len > 0.5) ratios[ax].push(len / ml);
      }
      const med = (arr) => (arr.length ? arr.slice().sort((a, b) => a - b)[Math.floor(arr.length / 2)] : null);
      const pairs = [["x", med(ratios.x)], ["y", med(ratios.y)], ["z", med(ratios.z)]].filter(([, v]) => v != null);
      const scale = Number(g?.attrs["data-scale"] || 0) || pairs[0]?.[1] || 1;
      if (bad.length) add("projection", "FAIL", `${name}: ${bad.length} axis-tagged edge(s) off the isometric axes: ${bad.slice(0, 3).join("; ")}`);
      else add("projection", "PASS", `${name}: all ${axisLines.length} axis edges lie on 30°/150°/90°`);
      const worst = Math.max(...pairs.map(([, v]) => Math.abs(v / scale - 1)));
      if (worst > 0.01) add("axis-scale", "FAIL", `${name}: axis scales differ by ${(worst * 100).toFixed(1)}% (${pairs.map(([k, v]) => `${k}=${v.toFixed(3)} mm/unit`).join(", ")})`);
      else add("axis-scale", "PASS", `${name}: equal axis scales (${pairs.map(([k, v]) => `${k}=${v.toFixed(3)}`).join(", ")} mm/unit)`);
    } else {
      const hist = new Map();
      let total = 0, onAxis = 0;
      for (const e of strokeEls.filter(inG)) for (const [p, q] of segmentsOf(e)) {
        const len = Math.hypot(q[0] - p[0], q[1] - p[1]);
        if (len < 0.5) continue;
        let ang = (Math.atan2(q[1] - p[1], q[0] - p[0]) * 180) / Math.PI;
        ang = ((ang % 180) + 180) % 180;
        hist.set(Math.round(ang), (hist.get(Math.round(ang)) || 0) + len);
        total += len;
        if ([30, 150, 90].some((t) => Math.abs(ang - t) <= 1)) onAxis += len;
      }
      const top = [...hist.entries()].sort((a, b) => b[1] - a[1]).slice(0, 4).map(([b, l]) => `${b}° (${((l / total) * 100).toFixed(0)}%)`);
      const frac = total ? onAxis / total : 0;
      add("projection", frac >= 0.6 ? "PASS" : "FAIL", `${name}: ${(frac * 100).toFixed(1)}% of stroke length on isometric axes; dominant angles ${top.join(", ")} (untagged drawing — tag edges with data-axis for an exact audit)`);
    }
  }

  // 2. proportion: drawn extents vs declared size
  for (const g of views) {
    const scale = Number(g.attrs["data-scale"] || 0);
    if (!scale) continue;
    const extents = els.filter((e) => e.tag === "rect" && hasClass(e, "extent") && inGroup(e, g));
    let fails = 0, partial = 0;
    for (const ex of extents) {
      const part = ex.attrs["data-part"];
      if (ex.attrs["data-partial"] === "1") { partial++; continue; }
      const [sw, sh] = (ex.attrs["data-size"] || "").split(/\s+/).map(Number);
      const lines = strokeEls.filter((e) => inGroup(e, g) && e.attrs["data-part"] === part && (hasClass(e, "obj") || hasClass(e, "hid") || hasClass(e, "brk") || hasClass(e, "cut")));
      if (!lines.length) continue;
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      for (const e of lines) for (const [p, q] of segmentsOf(e)) { x0 = Math.min(x0, p[0], q[0]); x1 = Math.max(x1, p[0], q[0]); y0 = Math.min(y0, p[1], q[1]); y1 = Math.max(y1, p[1], q[1]); }
      const ew = sw * scale, eh = sh * scale, dw = x1 - x0, dh = y1 - y0;
      const errW = Math.abs(dw - ew) / Math.max(ew, 1), errH = Math.abs(dh - eh) / Math.max(eh, 1);
      if ((errW > 0.01 && Math.abs(dw - ew) > 0.15) || (errH > 0.01 && Math.abs(dh - eh) > 0.15)) {
        fails++;
        add("proportion", "FAIL", `${g.attrs["data-view"]}: part ${part} drawn ${dw.toFixed(2)}×${dh.toFixed(2)} mm, model says ${ew.toFixed(2)}×${eh.toFixed(2)} mm (${(Math.max(errW, errH) * 100).toFixed(1)}% off)`);
      }
    }
    if (!fails) add("proportion", "PASS", `${g.attrs["data-view"]}: ${extents.length - partial} part extent(s) match the model within 1%${partial ? ` (${partial} partly occluded, silhouette not measurable)` : ""}`);
  }

  // 3. collisions
  const texts = els.filter((e) => e.tag === "text" && e.content).map((e) => textBox(e, unitToMm));
  const shrink = 0.3;
  const details = [];
  const ownGroups = (t) => t.el.groups.filter((g) => ["dim", "balloon", "title-block", "overlay", "cutting-plane", "scale-bar", "projection-symbol", "parts-list"].some((c) => (g.attrs.class || "").split(/\s+/).includes(c)));
  for (const t of texts) {
    const r = { x0: t.x0 + shrink, y0: t.y0 + shrink, x1: t.x1 - shrink, y1: t.y1 - shrink };
    if (r.x1 <= r.x0) continue;
    const own = ownGroups(t);
    for (const s of strokeEls) {
      if (hasClass(s, "border") || hasClass(s, "card") || hasClass(s, "align")) continue;
      if (own.length && own.some((g) => s.groups.some((sg) => sg.attrs === g.attrs))) continue;
      let hit = false;
      for (const [p, q] of segmentsOf(s)) if (segHitsRect(p, q, r)) { hit = true; break; }
      if (hit) details.push(`"${t.content}" crosses <${s.tag} class="${s.attrs.class || ""}">`);
    }
  }
  for (let i = 0; i < texts.length; i++) for (let j = i + 1; j < texts.length; j++) {
    const a = texts[i], b = texts[j];
    const ra = { x0: a.x0 + shrink, y0: a.y0 + shrink, x1: a.x1 - shrink, y1: a.y1 - shrink };
    const rb = { x0: b.x0 + shrink, y0: b.y0 + shrink, x1: b.x1 - shrink, y1: b.y1 - shrink };
    if (rectsOverlap(ra, rb)) details.push(`"${a.content}" overlaps "${b.content}"`);
  }
  if (details.length) add("collision", "FAIL", `${details.length} text collision(s): ${details.slice(0, 6).join("; ")}${details.length > 6 ? "; …" : ""}`, { items: details });
  else add("collision", "PASS", `${texts.length} text elements clear of strokes and each other`);

  // 4. dimensions
  const dims = els.filter((e) => e.tag === "g" && hasClass(e, "dim")).map((g) => {
    const t = els.find((e) => e.tag === "text" && inGroup(e, g));
    return { vid: g.attrs["data-vid"] || "", view: g.attrs["data-view"], axis: g.attrs["data-axis"], row: Number(g.attrs["data-row"] || 1), side: g.attrs["data-side"] || "", from: Number(g.attrs["data-from"]), to: Number(g.attrs["data-to"]), value: Number(g.attrs["data-value"]), label: t?.content || "" };
  });
  const dimProblems = [];
  for (const d of dims) {
    const parsed = num(d.label.replace(/[^\d\-./ ]/g, "").trim());
    if (Number.isFinite(parsed) && Math.abs(parsed - d.value) > tol) dimProblems.push(`${d.view}/${d.axis}: label "${d.label}" ≠ measured ${d.value}`);
    if (Number.isFinite(d.from) && Number.isFinite(d.to) && Math.abs(d.to - d.from - d.value) > tol) dimProblems.push(`${d.view}/${d.axis}: value ${d.value} ≠ to−from ${r3(d.to - d.from)}`);
  }
  const byKey = new Map();
  for (const d of dims) { const k = `${d.vid}|${d.view}|${d.axis}|${d.side}`; if (!byKey.has(k)) byKey.set(k, []); byKey.get(k).push(d); }
  let chains = 0;
  for (const list of byKey.values()) {
    const rows = new Map();
    for (const d of list) { if (!rows.has(d.row)) rows.set(d.row, []); rows.get(d.row).push(d); }
    for (const [row, ds] of rows) {
      ds.sort((a, b) => a.from - b.from);
      const runs = [];
      let run = [ds[0]];
      for (let i = 1; i < ds.length; i++) {
        if (Math.abs(ds[i].from - run[run.length - 1].to) <= tol) run.push(ds[i]); else { runs.push(run); run = [ds[i]]; }
      }
      runs.push(run);
      for (const r of runs) {
        if (r.length < 2) continue;
        const lo = r[0].from, hi = r[r.length - 1].to, sum = r.reduce((a, d) => a + d.value, 0);
        const overall = list.find((d) => d.row > row && Math.abs(d.from - lo) <= tol && Math.abs(d.to - hi) <= tol);
        if (!overall) continue;
        chains++;
        if (Math.abs(sum - overall.value) > tol) dimProblems.push(`${r[0].view}/${r[0].axis} row ${row}: chain ${r.map((d) => d.label).join(" + ")} = ${r3(sum)} but the overall reads ${overall.label}`);
      }
    }
  }
  if (dims.length) {
    if (dimProblems.length) add("dimensions", "FAIL", `${dimProblems.length} dimension defect(s): ${dimProblems.slice(0, 5).join("; ")}`, { items: dimProblems });
    else add("dimensions", "PASS", `${dims.length} dimensions consistent; ${chains} chain(s) sum to their overall`);
  }

  // 5. text size at print scale
  if (texts.length) {
    const smallest = texts.reduce((a, t) => (t.sizeMm < a.sizeMm ? t : a));
    const dimTexts = texts.filter((t) => t.el.groups.some((g) => (g.attrs.class || "").split(/\s+/).includes("dim")));
    const smallestDim = dimTexts.length ? dimTexts.reduce((a, t) => (t.sizeMm < a.sizeMm ? t : a)) : null;
    const probs = [];
    if (smallest.sizeMm < minText - 1e-9) probs.push(`smallest text "${smallest.content}" is ${smallest.sizeMm.toFixed(2)} mm on paper (floor ${minText} mm)`);
    if (smallestDim && smallestDim.sizeMm < minDimText - 1e-9) probs.push(`smallest dimension text "${smallestDim.content}" is ${smallestDim.sizeMm.toFixed(2)} mm (floor ${minDimText} mm)`);
    if (probs.length) add("text-size", "FAIL", probs.join("; "));
    else add("text-size", "PASS", `smallest text ${smallest.sizeMm.toFixed(2)} mm${smallestDim ? `, smallest dimension text ${smallestDim.sizeMm.toFixed(2)} mm` : ""} at print scale`);
  }
  return findings;
}

export function checkModel(model, opts = {}) {
  const findings = [];
  const add = (check, status, detail) => findings.push({ check, status, detail });
  const tol = model.units === "mm" ? 0.5 : 1 / 64;
  for (const p of model.problems) add("model", "FAIL", p);
  for (const p of model.parts) {
    const d = boxDims(p.box).slice().sort((a, b) => b - a);
    const s = p.size.slice().sort((a, b) => b - a);
    if (d.some((v, i) => Math.abs(v - s[i]) > tol)) add("cut-size", "FAIL", `${p.id}: placed extents ${d.map((v) => fmt(v, model.units)).join(" × ")} ≠ cut size ${s.map((v) => fmt(v, model.units)).join(" × ")}`);
  }
  if (!findings.some((f) => f.check === "cut-size")) add("cut-size", "PASS", `${model.parts.length} parts: placed extents match their cut sizes`);
  const jp = [];
  for (const j of model.joints) {
    if (!j.male) continue;
    if (j.stated.width != null && Math.abs(j.stated.width - j.width) > tol) jp.push(`${j.id}: stated width ${fmt(j.stated.width, model.units)} but the parts overlap ${fmt(j.width, model.units)}`);
    if (j.stated.depth != null && Math.abs(j.stated.depth - j.depth) > tol) jp.push(`${j.id}: stated depth ${fmt(j.stated.depth, model.units)} but the male enters ${fmt(j.depth, model.units)}`);
    const f = j.female.box;
    const edgeW = nearly(j.overlap.min[j.widthAxis], f.min[j.widthAxis], 1e-6) || nearly(j.overlap.max[j.widthAxis], f.max[j.widthAxis], 1e-6);
    if (j.type === "rabbet" && !edgeW) jp.push(`${j.id}: a rabbet sits at an edge of ${j.female.id}; this cut is interior (that is a dado)`);
    if (j.type === "dado" && edgeW) jp.push(`${j.id}: a dado is interior to ${j.female.id}; this cut is at its edge (that is a rabbet)`);
    if (j.depth >= boxDims(f)[j.axis] - EPS) jp.push(`${j.id}: the cut goes through the full thickness of ${j.female.id}`);
    if (j.tenon && j.tenonThickness >= j.width - EPS) jp.push(`${j.id}: tenon thickness ${fmt(j.tenonThickness, model.units)} leaves no shoulder on ${j.male.id}`);
  }
  if (jp.length) jp.forEach((d) => add("joints", "FAIL", d)); else add("joints", "PASS", `${model.joints.length} joint(s) match their stated geometry`);
  const explained = new Set();
  for (const j of model.joints) {
    if (j.male) explained.add([j.female.id, j.male.id].sort().join("|"));
    if (j.parts) explained.add(j.parts.map((p) => p.id).sort().join("|"));
    if (j.from) explained.add([j.from.id, j.into.id].sort().join("|"));
  }
  const inter = [];
  for (let i = 0; i < model.parts.length; i++) for (let k = i + 1; k < model.parts.length; k++) {
    const a = model.parts[i], b = model.parts[k];
    const o = boxIntersect(a.box, b.box);
    if (!o) continue;
    if (explained.has([a.id, b.id].sort().join("|"))) continue;
    const covered = [a, b].some((p) => p.cuts.some((c) => { const x = boxIntersect(c, o); return x && boxVolume(x) >= boxVolume(o) - EPS; }));
    if (covered) continue;
    inter.push(`${a.id} and ${b.id} interpenetrate by ${boxDims(o).map((v) => fmt(v, model.units)).join(" × ")} with no joint declared`);
  }
  if (inter.length) inter.forEach((d) => add("interference", "FAIL", d)); else add("interference", "PASS", "no undeclared interpenetration between parts");
  if (opts.cutlist) {
    const cl = JSON.parse(fs.readFileSync(opts.cutlist, "utf8"));
    const rows = partsTable(model);
    const cp = [];
    const byName = new Map((cl.parts || []).map((p) => [p.name, p]));
    for (const r of rows) {
      const c = byName.get(r.cutlist) || byName.get(r.name);
      if (!c) { cp.push(`model part "${r.name}" is not in the cut list`); continue; }
      const cs = [num(c.length), num(c.width), num(c.thickness)].sort((a, b) => b - a);
      const ms = r.size.slice().sort((a, b) => b - a);
      if (cs.some((v, i) => Math.abs(v - ms[i]) > tol)) cp.push(`"${r.name}": model ${ms.map((v) => fmt(v, model.units)).join(" × ")} vs cut list ${cs.map((v) => fmt(v, model.units)).join(" × ")}`);
      if (Number(c.qty) !== r.qty) cp.push(`"${r.name}": model qty ${r.qty} vs cut list qty ${c.qty}`);
      byName.delete(c.name);
    }
    for (const left of byName.keys()) cp.push(`cut-list part "${left}" is not modelled`);
    if (cp.length) cp.forEach((d) => add("cut-list", "FAIL", d)); else add("cut-list", "PASS", `${rows.length} part rows match ${path.basename(opts.cutlist)} in size and quantity`);
  }
  return findings;
}

function printFindings(findings, json) {
  if (json) { console.log(JSON.stringify(findings, null, 2)); return; }
  for (const f of findings) console.log(`${f.status.padEnd(4)} ${f.check.padEnd(12)} ${f.detail}`);
  const fails = findings.filter((f) => f.status === "FAIL").length;
  console.log(`\n${fails ? `${fails} FAIL` : "ALL CHECKS PASS"} · ${findings.length} findings`);
}

// ───────────────────────────── CLI ─────────────────────────────
function parseArgs(argv) {
  const o = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const k = a.slice(2);
      const next = argv[i + 1];
      if (k.startsWith("no-")) o[k.slice(3)] = false;
      else if (next == null || next.startsWith("--")) o[k] = true;
      else { o[k] = next; i++; }
    } else if (a === "-o") o.out = argv[++i];
    else o._.push(a);
  }
  return o;
}

function usage() {
  console.log(`draft.mjs — parametric drafting renderer + mechanical checks
  render model.json --out dir [--sheet letter|a4|tabloid|a3] [--variant name] [--only ortho,iso,section,exploded,joints,parts] [--scale 1:16] [--grid] [--plain] [--png] [--dpi 300] [--dxf]
  check  model.json [--cutlist cutlist.json] [--variant name] [--json]      model checks, then every rendered sheet
  check  drawing.svg [--json] [--print-width-mm N] [--min-text-mm 2] [--min-dim-text-mm 2.5]
  table  model.json [--format md|csv|json]
  dxf    model.json --view front|top|right|iso -o out.dxf`);
}

function main() {
  const o = parseArgs(process.argv.slice(2));
  const [cmd, file] = o._;
  if (!cmd || !file) { usage(); process.exit(cmd ? 2 : 0); }
  if (cmd === "render") {
    const model = loadModel(file);
    if (model.problems.length) { for (const p of model.problems) console.error(`model: ${p}`); process.exit(2); }
    const files = renderSheets(model, { ...o, grid: o.grid ?? model.grid });
    for (const w of writeOutputs(model, files, { ...o, out: o.out || "drawings" })) console.log(w);
    return;
  }
  if (cmd === "check") {
    let findings = [];
    if (file.endsWith(".svg")) {
      findings = checkSvg(fs.readFileSync(file, "utf8"), { printWidthMm: o["print-width-mm"] ? Number(o["print-width-mm"]) : null, minTextMm: o["min-text-mm"] != null ? Number(o["min-text-mm"]) : undefined, minDimTextMm: o["min-dim-text-mm"] != null ? Number(o["min-dim-text-mm"]) : undefined });
    } else {
      const model = loadModel(file);
      findings = checkModel(model, { cutlist: o.cutlist });
      if (!model.problems.length) {
        for (const f of renderSheets(model, { ...o, grid: o.grid ?? model.grid })) {
          for (const r of checkSvg(f.svg, { units: model.units, minTextMm: o["min-text-mm"] != null ? Number(o["min-text-mm"]) : undefined })) findings.push({ ...r, sheet: f.name, detail: `[${f.kind}] ${r.detail}` });
        }
      }
    }
    printFindings(findings, o.json);
    process.exit(findings.some((f) => f.status === "FAIL") ? 1 : 0);
  }
  if (cmd === "table") {
    const model = loadModel(file);
    const rows = partsTable(model);
    const f = o.format || "md";
    if (f === "json") console.log(JSON.stringify(rows, null, 2));
    else if (f === "csv") { console.log("item,qty,name,length,width,thickness,material"); for (const r of rows) console.log([r.item, r.qty, JSON.stringify(r.name), ...r.size.map((v) => fmt(v, model.units)), JSON.stringify(r.material)].join(",")); }
    else { console.log("| Item | Qty | Part | L × W × T | Material |\n| --- | --- | --- | --- | --- |"); for (const r of rows) console.log(`| ${r.item} | ${r.qty} | ${r.name} | ${r.size.map((v) => fmt(v, model.units)).join(" × ")} | ${r.material} |`); }
    return;
  }
  if (cmd === "dxf") {
    const model = loadModel(file);
    const out = dxfFor(model, variantParts(model, o.variant), o.view || "front");
    if (o.out) fs.writeFileSync(o.out, out); else process.stdout.write(out);
    return;
  }
  usage();
  process.exit(2);
}

if (process.argv[1] && path.resolve(process.argv[1]) === new URL(import.meta.url).pathname) main();
