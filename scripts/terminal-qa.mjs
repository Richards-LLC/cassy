#!/usr/bin/env node
// terminal-qa — the terminal visual-QA gate for CLI/TUI output (cas-4df0).
//
// Runs a command inside a real pty at several widths, captures the ANSI byte
// stream, and evaluates it against dark, light and Solarized palettes. The
// checks are mechanical: they catch the defects a reviewer skims past (a table
// row wrapped at 80 columns, a warning colour that vanishes on a light
// background, numbers that do not line up, box drawing sent to a C locale,
// colour under NO_COLOR, spinner redraws in a pipe, a --json stream that is not
// one document). The receipt line it prints is what the cas-cli-craft critique
// and the worker close checklist cite for CLI surfaces.
//
// Usage:
//   node scripts/terminal-qa.mjs [--label <name>] [--out <dir>] [--widths 80,120]
//        [--palettes dark,light,solarized-dark,solarized-light]
//        [--allowlist <file.json>] [--strict] [--escape-flag --full]
//        [--json-flag --json] [--timeout-ms 120000] -- <command> [args...]
//
// Runs: every width × palette in a pty (COLORFGBG hints the palette to
// theme-detecting programs), plus "c-locale" (LC_ALL=C at 80), "no-color"
// (NO_COLOR=1 at 80), "piped" (stdout is not a tty) and, with --json-flag,
// "json" (piped, flag appended).
//
// Checks (id → severity):
//   contrast                    fail   colored text < 4.5:1, colored 1–2 cell mark < 3:1
//   overflow                    fail   a line wider than the pty (a wrapped row)
//   word-split                  fail   a token hard-broken at the right edge: the
//                                      line fills the width, ends mid-token, and the
//                                      wider capture shows the token whole (warn
//                                      when no wider capture can confirm it)
//   truncation-without-escape   fail   a token ends in … or ... and the output
//                                      never names the escape flag (--full)
//   numeric-misalignment        fail   numbers in one table column with
//                                      different right edges
//   unicode-without-fallback    fail   bytes ≥ 0x80 emitted under LC_ALL=C
//   color-under-no-color        fail   any SGR other than reset with NO_COLOR=1
//   control-when-piped          fail   cursor movement/erase or \r redraws when
//                                      stdout is not a tty
//   color-when-piped            warn   SGR colour when stdout is not a tty
//   json-contract               fail   --json stdout is not exactly one document
//
// Receipt (first line of report.md, last line on stdout):
//   terminal-qa: PASS <label> · <n> runs · <fails> fail · <warns> warn · <allowed> allowed · <report.json>
//
// Exit codes: 0 PASS, 1 FAIL, 2 usage/error. The command's own exit status is
// recorded per run and never decides the verdict.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// ---------------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------------

const SOLARIZED_ANSI = [
  "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
  "#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
];

export const PALETTES = {
  dark: {
    name: "dark",
    scheme: "dark",
    bg: "#1e1e1e",
    fg: "#cccccc",
    ansi: [
      "#000000", "#cd3131", "#0dbc79", "#e5e510", "#2472c8", "#bc3fbc", "#11a8cd", "#e5e5e5",
      "#666666", "#f14c4c", "#23d18b", "#f5f543", "#3b8eea", "#d670d6", "#29b8db", "#e5e5e5",
    ],
  },
  light: {
    name: "light",
    scheme: "light",
    bg: "#ffffff",
    fg: "#333333",
    ansi: [
      "#000000", "#cd3131", "#00bc00", "#949800", "#0451a5", "#bc05bc", "#0598bc", "#555555",
      "#666666", "#cd3131", "#14ce14", "#b5ba00", "#0451a5", "#bc05bc", "#0598bc", "#a5a5a5",
    ],
  },
  "solarized-dark": {
    name: "solarized-dark",
    scheme: "dark",
    bg: "#002b36",
    fg: "#839496",
    ansi: SOLARIZED_ANSI,
  },
  "solarized-light": {
    name: "solarized-light",
    scheme: "light",
    bg: "#fdf6e3",
    fg: "#657b83",
    ansi: SOLARIZED_ANSI,
  },
};

// ---------------------------------------------------------------------------
// Colour math (WCAG 2.x)
// ---------------------------------------------------------------------------

export function hexToRgb(hex) {
  const h = hex.replace("#", "");
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16));
}

export function rgbToHex([r, g, b]) {
  return "#" + [r, g, b].map((c) => Math.round(c).toString(16).padStart(2, "0")).join("");
}

function channel(c) {
  const s = c / 255;
  return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
}

export function luminance(rgb) {
  const [r, g, b] = rgb.map(channel);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastRatio(a, b) {
  const la = luminance(typeof a === "string" ? hexToRgb(a) : a);
  const lb = luminance(typeof b === "string" ? hexToRgb(b) : b);
  const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

function mix(a, b, t) {
  const ra = hexToRgb(a);
  const rb = hexToRgb(b);
  return rgbToHex(ra.map((c, i) => c + (rb[i] - c) * t));
}

function xterm256(index) {
  if (index < 16) return null; // caller maps through the palette table
  if (index >= 232) {
    const v = 8 + (index - 232) * 10;
    return rgbToHex([v, v, v]);
  }
  const i = index - 16;
  const steps = [0, 95, 135, 175, 215, 255];
  return rgbToHex([steps[Math.floor(i / 36)], steps[Math.floor(i / 6) % 6], steps[i % 6]]);
}

/** Resolve one SGR colour spec to a hex string for the given palette. */
function resolveColor(spec, palette, { bold = false, isFg = false } = {}) {
  if (!spec) return null;
  if (spec.type === "rgb") return rgbToHex([spec.r, spec.g, spec.b]);
  if (spec.type === "ansi") {
    let idx = spec.index;
    if (isFg && bold && idx < 8) idx += 8;
    return palette.ansi[idx];
  }
  if (spec.type === "256") {
    if (spec.index < 16) return palette.ansi[spec.index];
    return xterm256(spec.index);
  }
  return null;
}

/** Effective foreground/background for a style under a palette. */
export function effectiveColors(style, palette) {
  let fg = resolveColor(style.fg, palette, { bold: style.bold, isFg: true }) ?? palette.fg;
  let bg = resolveColor(style.bg, palette) ?? palette.bg;
  if (style.reverse) [fg, bg] = [bg, fg];
  if (style.dim) fg = mix(fg, bg, 0.5);
  return { fg, bg };
}

// ---------------------------------------------------------------------------
// Display width (wcwidth approximation)
// ---------------------------------------------------------------------------

const ZERO_WIDTH = [
  [0x0300, 0x036f], [0x0483, 0x0489], [0x0591, 0x05bd], [0x0610, 0x061a], [0x064b, 0x065f],
  [0x1ab0, 0x1aff], [0x1dc0, 0x1dff], [0x200b, 0x200f], [0x2028, 0x202e], [0x2060, 0x2064],
  [0x20d0, 0x20ff], [0xfe00, 0xfe0f], [0xfe20, 0xfe2f], [0xfeff, 0xfeff], [0xe0100, 0xe01ef],
];

const WIDE = [
  [0x1100, 0x115f], [0x231a, 0x231b], [0x2329, 0x232a], [0x23e9, 0x23ec], [0x23f0, 0x23f0],
  [0x23f3, 0x23f3], [0x25fd, 0x25fe], [0x2614, 0x2615], [0x2648, 0x2653], [0x267f, 0x267f],
  [0x2693, 0x2693], [0x26a1, 0x26a1], [0x26aa, 0x26ab], [0x26bd, 0x26be], [0x26c4, 0x26c5],
  [0x26ce, 0x26ce], [0x26d4, 0x26d4], [0x26ea, 0x26ea], [0x26f2, 0x26f3], [0x26f5, 0x26f5],
  [0x26fa, 0x26fa], [0x26fd, 0x26fd], [0x2705, 0x2705], [0x270a, 0x270b], [0x2728, 0x2728],
  [0x274c, 0x274c], [0x274e, 0x274e], [0x2753, 0x2755], [0x2757, 0x2757], [0x2795, 0x2797],
  [0x27b0, 0x27b0], [0x27bf, 0x27bf], [0x2b1b, 0x2b1c], [0x2b50, 0x2b50], [0x2b55, 0x2b55],
  [0x2e80, 0x303e], [0x3041, 0x33ff], [0x3400, 0x4dbf], [0x4e00, 0x9fff], [0xa000, 0xa4cf],
  [0xa960, 0xa97f], [0xac00, 0xd7a3], [0xf900, 0xfaff], [0xfe10, 0xfe19], [0xfe30, 0xfe6f],
  [0xff00, 0xff60], [0xffe0, 0xffe6], [0x1f004, 0x1f004], [0x1f0cf, 0x1f0cf], [0x1f18e, 0x1f18e],
  [0x1f191, 0x1f19a], [0x1f200, 0x1f202], [0x1f210, 0x1f23b], [0x1f240, 0x1f248], [0x1f250, 0x1f251],
  [0x1f300, 0x1f64f], [0x1f680, 0x1f6ff], [0x1f7e0, 0x1f7eb], [0x1f90c, 0x1f9ff], [0x1fa70, 0x1faff],
  [0x20000, 0x2fffd], [0x30000, 0x3fffd],
];

function inRanges(cp, ranges) {
  for (const [lo, hi] of ranges) {
    if (cp < lo) return false;
    if (cp <= hi) return true;
  }
  return false;
}

export function charWidth(ch) {
  const cp = ch.codePointAt(0);
  if (cp < 0x20 || (cp >= 0x7f && cp < 0xa0)) return 0;
  if (cp >= 0x2500 && cp <= 0x257f) return 1; // box drawing: always one cell
  if (inRanges(cp, ZERO_WIDTH)) return 0;
  if (inRanges(cp, WIDE)) return 2;
  return 1;
}

export function displayWidth(text) {
  let w = 0;
  for (const ch of text) w += charWidth(ch);
  return w;
}

// ---------------------------------------------------------------------------
// ANSI parsing → lines of styled cells
// ---------------------------------------------------------------------------

const ESC = "\u001b";

function defaultStyle() {
  return { fg: null, bg: null, bold: false, dim: false, reverse: false, underline: false };
}

function styleKey(s) {
  return JSON.stringify([s.fg, s.bg, s.bold, s.dim, s.reverse]);
}

function styleIsColored(s) {
  return Boolean(s.fg || s.bg || s.reverse || s.dim);
}

/** Apply one SGR parameter list to a style, returning the new style. */
function applySgr(params, style) {
  const s = { ...style };
  if (params.length === 0) return defaultStyle();
  for (let i = 0; i < params.length; i += 1) {
    const p = params[i];
    if (p === 0) Object.assign(s, defaultStyle());
    else if (p === 1) s.bold = true;
    else if (p === 2) s.dim = true;
    else if (p === 4) s.underline = true;
    else if (p === 7) s.reverse = true;
    else if (p === 22) { s.bold = false; s.dim = false; }
    else if (p === 24) s.underline = false;
    else if (p === 27) s.reverse = false;
    else if (p >= 30 && p <= 37) s.fg = { type: "ansi", index: p - 30 };
    else if (p === 39) s.fg = null;
    else if (p >= 40 && p <= 47) s.bg = { type: "ansi", index: p - 40 };
    else if (p === 49) s.bg = null;
    else if (p >= 90 && p <= 97) s.fg = { type: "ansi", index: p - 90 + 8 };
    else if (p >= 100 && p <= 107) s.bg = { type: "ansi", index: p - 100 + 8 };
    else if (p === 38 || p === 48) {
      const target = p === 38 ? "fg" : "bg";
      if (params[i + 1] === 5) {
        s[target] = { type: "256", index: params[i + 2] ?? 0 };
        i += 2;
      } else if (params[i + 1] === 2) {
        s[target] = { type: "rgb", r: params[i + 2] ?? 0, g: params[i + 3] ?? 0, b: params[i + 4] ?? 0 };
        i += 4;
      }
    }
  }
  return s;
}

/** SGR sequences that only reset. */
function isResetSgr(params) {
  return params.length === 0 || params.every((p) => p === 0);
}

const CONTROL_FINALS = new Set(["A", "B", "C", "D", "G", "H", "J", "K", "f", "S", "T"]);

/**
 * Parse a raw capture into visual lines.
 *
 * Each line: { cells: [{ch, w, style}], redraws, controls: [seq] }. A carriage
 * return not followed by a newline overwrites the line from column 0 (the
 * progress-redraw idiom); a backspace removes the last cell; a tab pads to the
 * next multiple of 8. OSC/DCS strings are dropped. Non-SGR CSI sequences are
 * recorded on the line (for the piped check) and dropped from the text.
 */
export function parseAnsi(raw) {
  const text = Buffer.isBuffer(raw) ? raw.toString("utf8") : String(raw);
  const lines = [];
  let line = { cells: [], redraws: 0, controls: [], sgr: [] };
  let style = defaultStyle();
  const chars = Array.from(text);
  const n = chars.length;
  let i = 0;

  const col = () => line.cells.reduce((acc, c) => acc + c.w, 0);
  const pushLine = () => {
    lines.push(line);
    line = { cells: [], redraws: 0, controls: [], sgr: [] };
  };

  while (i < n) {
    const ch = chars[i];
    if (ch === ESC) {
      const next = chars[i + 1];
      if (next === "[") {
        let j = i + 2;
        let params = "";
        while (j < n && !(chars[j] >= "@" && chars[j] <= "~")) {
          params += chars[j];
          j += 1;
        }
        const final = chars[j] ?? "";
        const seq = ESC + "[" + params + final;
        if (final === "m") {
          const list = params === "" ? [] : params.split(/[;:]/).map((p) => (p === "" ? 0 : Number(p)));
          line.sgr.push({ params: list, seq });
          style = applySgr(list, style);
        } else {
          line.controls.push(seq);
        }
        i = j + 1;
        continue;
      }
      if (next === "]" || next === "P" || next === "X" || next === "^" || next === "_") {
        // OSC / DCS / SOS / PM / APC: terminated by BEL or ESC \
        let j = i + 2;
        while (j < n) {
          if (chars[j] === "\u0007") { j += 1; break; }
          if (chars[j] === ESC && chars[j + 1] === "\\") { j += 2; break; }
          j += 1;
        }
        i = j;
        continue;
      }
      if (next === "(" || next === ")" || next === "*" || next === "+" || next === "#") {
        i += 3;
        continue;
      }
      i += 2;
      continue;
    }
    if (ch === "\n") { pushLine(); i += 1; continue; }
    if (ch === "\r") {
      if (chars[i + 1] === "\n" || i + 1 === n) { i += 1; continue; }
      if (line.cells.length > 0) {
        line.redraws += 1;
        line.cells = [];
      }
      i += 1;
      continue;
    }
    if (ch === "\b") { line.cells.pop(); i += 1; continue; }
    if (ch === "\t") {
      const c = col();
      const pad = 8 - (c % 8);
      for (let k = 0; k < pad; k += 1) line.cells.push({ ch: " ", w: 1, style });
      i += 1;
      continue;
    }
    if (ch === "\u0007") { i += 1; continue; }
    const w = charWidth(ch);
    if (w === 0 && line.cells.length > 0 && ch.codePointAt(0) >= 0x300) {
      line.cells[line.cells.length - 1].ch += ch; // combining mark
      i += 1;
      continue;
    }
    line.cells.push({ ch, w, style });
    i += 1;
  }
  if (line.cells.length > 0 || line.controls.length > 0 || line.sgr.length > 0 || lines.length === 0) {
    pushLine();
  }
  return lines;
}

export function lineText(line) {
  return line.cells.map((c) => c.ch).join("");
}

export function lineWidth(line) {
  return line.cells.reduce((acc, c) => acc + c.w, 0);
}

export function stripAnsi(raw) {
  return parseAnsi(raw).map(lineText).join("\n") + "\n";
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

function excerptOf(text) {
  const t = text.replace(/\s+$/, "");
  return t.length > 80 ? t.slice(0, 79) + "…" : t;
}

function finding(check, severity, run, lineNo, column, excerpt, detail, fix) {
  return { check, severity, run, line: lineNo, column, excerpt, detail, fix };
}

/** Split a line into table cells on 2+ spaces or │ / | separators. */
function tableCells(line) {
  const cells = [];
  let col = 0;
  let current = null;
  let spaceRun = 0;
  const flush = () => {
    if (current && current.text.trim() !== "") {
      const trimmedEnd = current.text.replace(/\s+$/, "");
      cells.push({ text: trimmedEnd.trim(), start: current.start, end: current.start + displayWidth(trimmedEnd) });
    }
    current = null;
  };
  for (const cell of line.cells) {
    const isSep = cell.ch === "│" || cell.ch === "|";
    if (isSep) { flush(); spaceRun = 0; col += cell.w; continue; }
    if (cell.ch === " ") {
      spaceRun += 1;
      if (spaceRun >= 2) {
        if (current) {
          // the previous single space belonged to the separator, drop it
          current.text = current.text.replace(/ $/, "");
        }
        flush();
      } else if (current) {
        current.text += " ";
      }
      col += 1;
      continue;
    }
    spaceRun = 0;
    if (!current) current = { text: "", start: col };
    current.text += cell.ch;
    col += cell.w;
  }
  flush();
  return cells;
}

const NUMERIC = /^[+-]?\d[\d,]*(\.\d+)?\s?(%|ms|s|m|h|d|[KMGT]i?B|B|x)?$/;

function checkContrast(lines, palette, run) {
  const out = [];
  lines.forEach((line, idx) => {
    let group = null;
    const groups = [];
    for (const cell of line.cells) {
      const key = styleKey(cell.style);
      if (!group || group.key !== key) {
        group = { key, style: cell.style, cells: [], start: lineWidth({ cells: line.cells.slice(0, line.cells.indexOf(cell)) }) };
        groups.push(group);
      }
      group.cells.push(cell);
    }
    for (const g of groups) {
      if (!styleIsColored(g.style)) continue;
      const visible = g.cells.filter((c) => c.ch.trim() !== "");
      const cellsCount = visible.reduce((acc, c) => acc + c.w, 0);
      if (cellsCount === 0) continue;
      const { fg, bg } = effectiveColors(g.style, palette);
      const ratio = contrastRatio(fg, bg);
      const kind = cellsCount >= 3 ? "text" : "mark";
      const floor = kind === "text" ? 4.5 : 3;
      if (ratio < floor) {
        const sample = g.cells.map((c) => c.ch).join("").trim();
        out.push(
          finding(
            "contrast",
            "fail",
            run,
            idx + 1,
            g.start + 1,
            excerptOf(lineText(line)),
            { kind, sample: sample.slice(0, 40), fg, bg, ratio: Number(ratio.toFixed(2)), floor, palette: palette.name, bold: g.style.bold, dim: g.style.dim },
            `${kind} "${sample.slice(0, 20)}" is ${fg} on ${bg} (${ratio.toFixed(2)}:1) under ${palette.name}; needs ≥ ${floor}:1 — pick a mid-tone that survives both backgrounds or leave body text in the default foreground`,
          ),
        );
      }
    }
  });
  return out;
}

function checkOverflow(lines, columns, run) {
  const out = [];
  lines.forEach((line, idx) => {
    const w = lineWidth(line);
    if (w > columns) {
      out.push(
        finding("overflow", "fail", run, idx + 1, columns + 1, excerptOf(lineText(line)), { width: w, columns },
          `line is ${w} cells wide in a ${columns}-column terminal and will wrap — truncate with … and offer --full, or drop a column`),
      );
    }
  });
  return out;
}

function tokens(text) {
  return text.split(/\s+/).filter(Boolean);
}

function checkWordSplit(lines, columns, run, referenceText) {
  const out = [];
  const refTokens = referenceText ? new Set(tokens(referenceText)) : null;
  for (let idx = 0; idx + 1 < lines.length; idx += 1) {
    const line = lines[idx];
    const next = lines[idx + 1];
    if (lineWidth(line) !== columns) continue;
    const text = lineText(line);
    const nextText = lineText(next);
    if (!/[A-Za-z0-9_/.-]$/.test(text)) continue;
    if (!/^\s*[A-Za-z0-9_]/.test(nextText)) continue;
    const last = tokens(text).at(-1);
    const first = tokens(nextText)[0];
    if (!last || !first) continue;
    const joined = last + first;
    let confirmed = null;
    if (refTokens) {
      const lastWhole = refTokens.has(last);
      let longer = false;
      for (const t of refTokens) {
        if (t.length > last.length && t.startsWith(last)) { longer = true; break; }
      }
      confirmed = !lastWhole && (refTokens.has(joined) || longer);
      if (!confirmed) continue;
    }
    out.push(
      finding("word-split", confirmed === null ? "warn" : "fail", run, idx + 1, columns, excerptOf(text),
        { last, first, joined: joined.slice(0, 80), confirmed: confirmed === true },
        `"${last.slice(-20)}" continues as "${first.slice(0, 20)}" on the next line — wrap on whitespace or truncate with … and offer --full`),
    );
  }
  return out;
}

function checkTruncation(lines, run, escapeFlag) {
  const out = [];
  const full = lines.map(lineText).join("\n");
  if (full.includes(escapeFlag)) return out;
  lines.forEach((line, idx) => {
    const text = lineText(line);
    for (const tok of tokens(text)) {
      if (/(…|\.\.\.)$/.test(tok)) {
        out.push(
          finding("truncation-without-escape", "fail", run, idx + 1, text.indexOf(tok) + 1, excerptOf(text),
            { token: tok.slice(0, 60), escape_flag: escapeFlag },
            `"${tok.slice(0, 30)}" is truncated but the output never mentions ${escapeFlag} — name the escape next to the ellipsis or in the footer`),
        );
        break;
      }
    }
  });
  return out;
}

function checkNumericAlignment(lines, run) {
  const out = [];
  const rows = lines.map((line, idx) => ({ idx, cells: tableCells(line), empty: lineText(line).trim() === "" }));
  let block = [];
  const flush = () => {
    if (block.length >= 2) {
      const columnsCount = Math.max(...block.map((r) => r.cells.length));
      for (let c = 0; c < columnsCount; c += 1) {
        const numeric = block
          .map((r) => ({ row: r, cell: r.cells[c] }))
          .filter(({ cell }) => cell && NUMERIC.test(cell.text));
        if (numeric.length < 2) continue;
        const ends = new Set(numeric.map(({ cell }) => cell.end));
        if (ends.size > 1) {
          const first = numeric[0];
          out.push(
            finding("numeric-misalignment", "fail", run, first.row.idx + 1, first.cell.start + 1, excerptOf(lineText(lines[first.row.idx])),
              {
                column: c + 1,
                lines: numeric.map(({ row }) => row.idx + 1),
                right_edges: numeric.map(({ cell }) => cell.end),
                values: numeric.map(({ cell }) => cell.text),
              },
              `column ${c + 1} holds numbers whose right edges differ (${[...ends].join(", ")}) on lines ${numeric.map(({ row }) => row.idx + 1).join(", ")} — right-align numeric columns`),
          );
        }
      }
    }
    block = [];
  };
  for (const r of rows) {
    if (!r.empty && r.cells.length >= 2) block.push(r);
    else flush();
  }
  flush();
  return out;
}

function checkUnicodeFallback(rawBuffer, run) {
  const out = [];
  const buf = Buffer.isBuffer(rawBuffer) ? rawBuffer : Buffer.from(String(rawBuffer), "utf8");
  let lineNo = 1;
  let lineStart = 0;
  for (let i = 0; i < buf.length; i += 1) {
    if (buf[i] === 0x0a) { lineNo += 1; lineStart = i + 1; continue; }
    if (buf[i] >= 0x80) {
      let lineEnd = buf.indexOf(0x0a, i);
      if (lineEnd === -1) lineEnd = buf.length;
      const lineStr = stripAnsi(buf.subarray(lineStart, lineEnd)).replace(/\r?\n$/, "");
      const cps = [...new Set(Array.from(lineStr).filter((ch) => ch.codePointAt(0) >= 0x80).map((ch) => "U+" + ch.codePointAt(0).toString(16).toUpperCase().padStart(4, "0")))];
      out.push(
        finding("unicode-without-fallback", "fail", run, lineNo, i - lineStart + 1, excerptOf(lineStr),
          { codepoints: cps.slice(0, 8), byte_offset: i },
          `non-ASCII output (${cps.slice(0, 4).join(" ")}) under LC_ALL=C — fall back to ASCII marks and +-| rules when the locale is not UTF-8`),
      );
      break;
    }
  }
  return out;
}

function checkNoColor(lines, run) {
  const out = [];
  lines.forEach((line, idx) => {
    const bad = line.sgr.find((s) => !isResetSgr(s.params));
    if (bad) {
      out.push(
        finding("color-under-no-color", "fail", run, idx + 1, 1, excerptOf(lineText(line)),
          { sequence: JSON.stringify(bad.seq) },
          `SGR ${JSON.stringify(bad.seq)} emitted with NO_COLOR=1 — honour NO_COLOR (https://no-color.org) by emitting no styling`),
      );
    }
  });
  return out;
}

function checkPiped(lines, run) {
  const out = [];
  lines.forEach((line, idx) => {
    const control = line.controls.find((seq) => {
      const final = seq.at(-1);
      return CONTROL_FINALS.has(final) || /^\u001b\[\?25[hl]$/.test(seq);
    });
    if (control || line.redraws > 0) {
      out.push(
        finding("control-when-piped", "fail", run, idx + 1, 1, excerptOf(lineText(line)),
          { sequence: control ? JSON.stringify(control) : null, redraws: line.redraws },
          `${control ? "cursor/erase sequence " + JSON.stringify(control) : line.redraws + " carriage-return redraw(s)"} with stdout not a tty — print one line per event when piped`),
    );
    }
  });
  const colored = lines.findIndex((line) => line.sgr.some((s) => !isResetSgr(s.params)));
  if (colored !== -1) {
    out.push(
      finding("color-when-piped", "warn", run, colored + 1, 1, excerptOf(lineText(lines[colored])),
        { sequence: JSON.stringify(lines[colored].sgr.find((s) => !isResetSgr(s.params)).seq) },
        "colour emitted while stdout is not a tty — drop styling when piped unless --color=always is given"),
    );
  }
  return out;
}

function checkJsonContract(raw, run, jsonFlag) {
  const text = Buffer.isBuffer(raw) ? raw.toString("utf8") : String(raw);
  const trimmed = text.trim();
  try {
    if (trimmed === "") throw new Error("empty stdout");
    JSON.parse(trimmed);
    return [];
  } catch (err) {
    const firstLine = trimmed.split("\n")[0] ?? "";
    return [
      finding("json-contract", "fail", run, 1, 1, excerptOf(firstLine), { error: err.message, flag: jsonFlag },
        `stdout with ${jsonFlag} is not exactly one JSON document (${err.message}) — send logs and progress to stderr and print one document`),
    ];
  }
}

/**
 * Analyze one capture.
 *
 * @param {object} opts
 * @param {Buffer|string} opts.raw           captured bytes
 * @param {number} opts.columns              pty width the run used
 * @param {object|string} opts.palette       PALETTES entry or its name
 * @param {object|string} opts.run           { name, kind } or a run name; kind is one of
 *                                           pty | c-locale | no-color | piped | json (default pty)
 * @param {string} [opts.escapeFlag]         truncation escape flag (default --full)
 * @param {string} [opts.jsonFlag]           flag used for the json run (default --json)
 * @param {string} [opts.referenceText]      plain text of a wider capture, to confirm word splits
 */
export function analyzeCapture({ raw, columns = 80, palette = "dark", run = "pty", escapeFlag = "--full", jsonFlag = "--json", referenceText = null }) {
  const pal = typeof palette === "string" ? PALETTES[palette] : palette;
  const runName = typeof run === "string" ? run : run.name;
  const kind = typeof run === "string" ? "pty" : run.kind ?? "pty";
  const lines = parseAnsi(raw);
  if (kind === "c-locale") return checkUnicodeFallback(raw, runName);
  if (kind === "no-color") return checkNoColor(lines, runName);
  if (kind === "piped") return checkPiped(lines, runName);
  if (kind === "json") return checkJsonContract(raw, runName, jsonFlag);
  return [
    ...checkContrast(lines, pal, runName),
    ...checkOverflow(lines, columns, runName),
    ...checkWordSplit(lines, columns, runName, referenceText),
    ...checkTruncation(lines, runName, escapeFlag),
    ...checkNumericAlignment(lines, runName),
  ];
}

// ---------------------------------------------------------------------------
// Renderers
// ---------------------------------------------------------------------------

function escapeHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function renderCaptureHtml(raw, palette, width) {
  const lines = parseAnsi(raw);
  const body = lines
    .map((line) => {
      let html = "";
      let key = null;
      let buf = "";
      let style = null;
      const flush = () => {
        if (buf === "") return;
        if (style && styleIsColored(style) || (style && style.bold)) {
          const { fg, bg } = effectiveColors(style, palette);
          const css = [`color:${fg}`];
          if (style.bg || style.reverse) css.push(`background:${bg}`);
          if (style.bold) css.push("font-weight:bold");
          if (style.underline) css.push("text-decoration:underline");
          html += `<span style="${css.join(";")}">${escapeHtml(buf)}</span>`;
        } else {
          html += escapeHtml(buf);
        }
        buf = "";
      };
      for (const cell of line.cells) {
        const k = styleKey(cell.style) + (cell.style.underline ? "u" : "");
        if (k !== key) { flush(); key = k; style = cell.style; }
        buf += cell.ch;
      }
      flush();
      return html;
    })
    .join("\n");
  return `<pre style="background:${palette.bg};color:${palette.fg};width:${width}ch;padding:12px;margin:0 0 24px 0;overflow-x:auto;font:13px/1.35 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;border:1px solid #8884">${body}</pre>`;
}

function renderWidthHtml(label, width, runs) {
  const sections = runs
    .map((run) => `<h2 style="font:600 14px system-ui;margin:0 0 6px 0">${escapeHtml(run.palette)} · ${width} columns · exit ${run.exit_code}${run.max_width > width ? ` · <span style="color:#b00">max line ${run.max_width}</span>` : ""}</h2>\n${renderCaptureHtml(run.raw, PALETTES[run.palette], width)}`)
    .join("\n");
  return `<!doctype html>
<meta charset="utf-8">
<title>terminal-qa · ${escapeHtml(label)} · ${width} columns</title>
<body style="margin:24px;background:#f3f1ec;color:#222;font:14px system-ui">
<h1 style="font:600 18px system-ui;margin:0 0 16px 0">terminal-qa · ${escapeHtml(label)} · ${width} columns</h1>
<p style="margin:0 0 16px 0;max-width:80ch">Each block is the same capture rendered on one palette's background with that palette's ANSI table. Anything you cannot read here, a user cannot read either.</p>
${sections}
</body>
`;
}

function mdCell(s) {
  return String(s ?? "").replace(/\|/g, "\\|").replace(/\n/g, " ");
}

function receiptLine(report) {
  return `terminal-qa: ${report.verdict} ${report.label} · ${report.runs.length} runs · ${report.counts.fail} fail · ${report.counts.warn} warn · ${report.counts.allowed} allowed · ${report.report_json}`;
}

function renderReportMd(report) {
  const lines = [receiptLine(report), ""];
  lines.push(`Command: \`${report.command.join(" ")}\`  `);
  lines.push(`Generated: ${report.generated_at}`, "");
  lines.push("## Findings", "");
  if (report.findings.length === 0) lines.push("None.", "");
  else {
    lines.push("| # | Check | Severity | Run | Line | Excerpt | Detail |", "| --- | --- | --- | --- | ---: | --- | --- |");
    report.findings.forEach((f, i) => {
      lines.push(`| ${i + 1} | ${f.check} | ${f.severity} | ${f.run} | ${f.line} | \`${mdCell(f.excerpt)}\` | ${mdCell(f.fix)} |`);
    });
    lines.push("");
  }
  lines.push("## Allowed", "");
  if (report.allowed.length === 0) lines.push("None.", "");
  else {
    lines.push("| Check | Run | Line | Excerpt | Reason |", "| --- | --- | ---: | --- | --- |");
    for (const f of report.allowed) lines.push(`| ${f.check} | ${f.run} | ${f.line} | \`${mdCell(f.excerpt)}\` | ${mdCell(f.reason)} |`);
    lines.push("");
  }
  lines.push("## Runs", "");
  lines.push("| Run | Width | Palette | Exit | Lines | Max width | Capture |", "| --- | ---: | --- | ---: | ---: | ---: | --- |");
  for (const r of report.runs) {
    lines.push(`| ${r.name} | ${r.width ?? "—"} | ${r.palette ?? "—"} | ${r.exit_code} | ${r.lines} | ${r.max_width} | ${r.files.txt} |`);
  }
  lines.push("");
  return lines.join("\n");
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

function shellQuote(arg) {
  return "'" + String(arg).replace(/'/g, "'\\''") + "'";
}

function baseEnv() {
  const env = { ...process.env };
  delete env.NO_COLOR;
  delete env.COLORFGBG;
  delete env.CLICOLOR_FORCE;
  delete env.FORCE_COLOR;
  env.TERM = "xterm-256color";
  return env;
}

function runInPty(command, args, { columns, env, timeoutMs }) {
  const inner = `stty cols ${columns} rows 50 2>/dev/null; exec ${[command, ...args].map(shellQuote).join(" ")}`;
  const res = spawnSync("script", ["-q", "-e", "-c", inner, "/dev/null"], {
    env,
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 64 * 1024 * 1024,
    timeout: timeoutMs,
  });
  if (res.error) throw res.error;
  return { raw: res.stdout ?? Buffer.alloc(0), stderr: res.stderr ?? Buffer.alloc(0), exit_code: res.status ?? (res.signal ? 128 : 1), timed_out: Boolean(res.error?.code === "ETIMEDOUT") };
}

function runPiped(command, args, { env, timeoutMs }) {
  const res = spawnSync(command, args, {
    env,
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 64 * 1024 * 1024,
    timeout: timeoutMs,
  });
  if (res.error) throw res.error;
  return { raw: res.stdout ?? Buffer.alloc(0), stderr: res.stderr ?? Buffer.alloc(0), exit_code: res.status ?? (res.signal ? 128 : 1) };
}

function slugify(s) {
  return s.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 80) || "command";
}

function relPath(p) {
  const r = path.relative(process.cwd(), p);
  return r && !r.startsWith("..") ? r : p;
}

function loadAllowlist(file) {
  if (!file) return [];
  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  if (!Array.isArray(data)) throw new UsageError(`allowlist ${file} must be a JSON array`);
  data.forEach((entry, i) => {
    if (!entry || typeof entry !== "object") throw new UsageError(`allowlist entry ${i} is not an object`);
    if (!entry.check) throw new UsageError(`allowlist entry ${i} has no "check"`);
    if (typeof entry.reason !== "string" || entry.reason.trim() === "") throw new UsageError(`allowlist entry ${i} (${entry.check}) has no reason — every exception must say why`);
  });
  return data;
}

function allowMatcher(entry) {
  const m = entry.match;
  if (m === undefined || m === null || m === "") return () => true;
  const re = typeof m === "string" && /^\/.*\/[a-z]*$/.test(m) ? new RegExp(m.slice(1, m.lastIndexOf("/")), m.slice(m.lastIndexOf("/") + 1)) : null;
  return (f) => {
    const hay = [f.excerpt ?? "", JSON.stringify(f.detail ?? {}), f.run ?? "", f.fix ?? ""];
    return re ? hay.some((h) => re.test(h)) : hay.some((h) => h.includes(String(m)));
  };
}

export class UsageError extends Error {}

/**
 * Run the full gate.
 *
 * @param {object} options
 * @param {string[]} options.command        command and args
 * @param {string} [options.label]
 * @param {string} [options.out]
 * @param {number[]} [options.widths]
 * @param {string[]} [options.palettes]
 * @param {string} [options.allowlist]      path to allowlist JSON
 * @param {boolean} [options.strict]
 * @param {string} [options.escapeFlag]
 * @param {string} [options.jsonFlag]
 * @param {number} [options.timeoutMs]
 * @param {boolean} [options.quiet]         suppress stdout printing
 */
export async function runTerminalQa(options) {
  const {
    command,
    widths = [80, 120],
    palettes = Object.keys(PALETTES),
    strict = false,
    escapeFlag = "--full",
    jsonFlag = null,
    timeoutMs = 120000,
    quiet = true,
  } = options;
  if (!command || command.length === 0) throw new UsageError("no command given after --");
  for (const p of palettes) if (!PALETTES[p]) throw new UsageError(`unknown palette "${p}" (known: ${Object.keys(PALETTES).join(", ")})`);
  for (const w of widths) if (!Number.isInteger(w) || w < 20) throw new UsageError(`invalid width "${w}"`);
  const which = spawnSync("sh", ["-c", "command -v script"], { stdio: ["ignore", "pipe", "ignore"] });
  if (which.status !== 0) throw new UsageError("util-linux `script` is required to allocate a pty and was not found on PATH");

  const [cmd, ...args] = command;
  const label = options.label ?? slugify([path.basename(cmd), ...args].join("-"));
  const outDir = path.resolve(options.out ?? path.join(".cas", "artifacts", "terminal-qa", label));
  fs.mkdirSync(outDir, { recursive: true });
  const allowlist = loadAllowlist(options.allowlist);

  const runs = [];
  const captures = new Map();
  const record = (name, extra, result) => {
    const lines = parseAnsi(result.raw);
    const ansiFile = path.join(outDir, `${label}.${name}.ansi`);
    const txtFile = path.join(outDir, `${label}.${name}.txt`);
    fs.writeFileSync(ansiFile, result.raw);
    fs.writeFileSync(txtFile, stripAnsi(result.raw));
    const run = {
      name,
      ...extra,
      exit_code: result.exit_code,
      lines: lines.length,
      max_width: Math.max(0, ...lines.map(lineWidth)),
      files: { ansi: relPath(ansiFile), txt: relPath(txtFile) },
    };
    runs.push(run);
    captures.set(name, { run, raw: result.raw });
    return run;
  };

  // The piped run goes first: a missing command surfaces as ENOENT here.
  try {
    record("piped", { width: null, palette: null, env: {}, kind: "piped" }, runPiped(cmd, args, { env: baseEnv(), timeoutMs }));
  } catch (err) {
    if (err.code === "ENOENT") throw new UsageError(`command not found: ${cmd}`);
    throw err;
  }
  for (const width of widths) {
    for (const paletteName of palettes) {
      const palette = PALETTES[paletteName];
      const env = baseEnv();
      env.COLORFGBG = palette.scheme === "dark" ? "15;0" : "0;15";
      record(`${width}.${paletteName}`, { width, palette: paletteName, env: { COLORFGBG: env.COLORFGBG, TERM: env.TERM }, kind: "pty" }, runInPty(cmd, args, { columns: width, env, timeoutMs }));
    }
  }
  {
    const env = baseEnv();
    env.LC_ALL = "C";
    env.LANG = "C";
    env.LANGUAGE = "C";
    record("c-locale", { width: 80, palette: null, env: { LC_ALL: "C", LANG: "C", LANGUAGE: "C" }, kind: "c-locale" }, runInPty(cmd, args, { columns: 80, env, timeoutMs }));
  }
  {
    const env = baseEnv();
    env.NO_COLOR = "1";
    record("no-color", { width: 80, palette: null, env: { NO_COLOR: "1" }, kind: "no-color" }, runInPty(cmd, args, { columns: 80, env, timeoutMs }));
  }
  if (jsonFlag) {
    record("json", { width: null, palette: null, env: {}, kind: "json", flag: jsonFlag }, runPiped(cmd, [...args, jsonFlag], { env: baseEnv(), timeoutMs }));
  }

  // Analysis. Narrow pty runs get the widest same-palette capture as a word-split reference.
  const widest = Math.max(...widths);
  const findingsAll = [];
  for (const { run, raw } of captures.values()) {
    let referenceText = null;
    if (run.kind === "pty" && run.width !== widest) {
      const ref = captures.get(`${widest}.${run.palette}`);
      if (ref) referenceText = stripAnsi(ref.raw);
    }
    findingsAll.push(
      ...analyzeCapture({ raw, columns: run.width ?? 80, palette: run.palette ?? "dark", run: { name: run.name, kind: run.kind }, escapeFlag, jsonFlag: jsonFlag ?? "--json", referenceText }),
    );
  }

  const findings = [];
  const allowed = [];
  for (const f of findingsAll) {
    const entry = allowlist.find((e) => (e.check === "*" || e.check === f.check) && allowMatcher(e)(f));
    if (entry) allowed.push({ ...f, reason: entry.reason });
    else findings.push(f);
  }
  const counts = {
    fail: findings.filter((f) => f.severity === "fail").length,
    warn: findings.filter((f) => f.severity === "warn").length,
    allowed: allowed.length,
  };
  const verdict = counts.fail > 0 || (strict && counts.warn > 0) ? "FAIL" : "PASS";

  // HTML per width, every palette stacked.
  const html = {};
  for (const width of widths) {
    const file = path.join(outDir, `${label}.${width}.html`);
    const widthRuns = palettes.map((p) => ({ ...captures.get(`${width}.${p}`).run, raw: captures.get(`${width}.${p}`).raw }));
    fs.writeFileSync(file, renderWidthHtml(label, width, widthRuns));
    html[width] = relPath(file);
  }

  const reportJsonPath = path.join(outDir, "report.json");
  const reportMdPath = path.join(outDir, "report.md");
  const report = {
    label,
    verdict,
    command,
    widths,
    palettes,
    strict,
    escape_flag: escapeFlag,
    json_flag: jsonFlag,
    runs: runs.map((r) => ({ ...r })),
    findings,
    allowed,
    counts,
    html,
    report_json: relPath(reportJsonPath),
    report_md: relPath(reportMdPath),
    generated_at: new Date().toISOString(),
  };
  report.receipt = receiptLine(report);
  fs.writeFileSync(reportJsonPath, JSON.stringify(report, null, 2) + "\n");
  fs.writeFileSync(reportMdPath, renderReportMd(report));

  if (!quiet) {
    for (const f of findings) {
      process.stdout.write(`${f.severity.toUpperCase().padEnd(4)} ${f.check}  ${f.run}:${f.line}  ${f.fix}\n       ${f.excerpt}\n`);
    }
    for (const f of allowed) {
      process.stdout.write(`allow ${f.check}  ${f.run}:${f.line}  ${f.reason}\n`);
    }
    process.stdout.write(report.receipt + "\n");
  }
  return report;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

const USAGE = `usage: terminal-qa.mjs [--label <name>] [--out <dir>] [--widths 80,120]
        [--palettes dark,light,solarized-dark,solarized-light] [--allowlist <file.json>]
        [--strict] [--escape-flag --full] [--json-flag --json] [--timeout-ms 120000]
        -- <command> [args...]`;

export function parseArgs(argv) {
  const opts = { command: [] };
  let i = 0;
  while (i < argv.length) {
    const a = argv[i];
    if (a === "--") { opts.command = argv.slice(i + 1); break; }
    const next = () => {
      const v = argv[i + 1];
      if (v === undefined) throw new UsageError(`${a} needs a value`);
      i += 1;
      return v;
    };
    if (a === "--help" || a === "-h") { opts.help = true; }
    else if (a === "--label") opts.label = next();
    else if (a === "--out") opts.out = next();
    else if (a === "--widths") opts.widths = next().split(",").map((w) => Number(w.trim()));
    else if (a === "--palettes") opts.palettes = next().split(",").map((p) => p.trim()).filter(Boolean);
    else if (a === "--allowlist") opts.allowlist = next();
    else if (a === "--strict") opts.strict = true;
    else if (a === "--escape-flag") opts.escapeFlag = next();
    else if (a === "--json-flag") opts.jsonFlag = next();
    else if (a === "--timeout-ms") opts.timeoutMs = Number(next());
    else throw new UsageError(`unknown option ${a}`);
    i += 1;
  }
  return opts;
}

async function main() {
  let opts;
  try {
    opts = parseArgs(process.argv.slice(2));
    if (opts.help) {
      process.stdout.write(USAGE + "\n");
      return 0;
    }
    if (opts.command.length === 0) throw new UsageError("no command given after --");
    const report = await runTerminalQa({ ...opts, quiet: false });
    return report.verdict === "PASS" ? 0 : 1;
  } catch (err) {
    if (err instanceof UsageError || err?.code === "ENOENT") {
      process.stderr.write(`terminal-qa: error: ${err.message}\n${USAGE}\n`);
      return 2;
    }
    process.stderr.write(`terminal-qa: error: ${err?.stack ?? err}\n`);
    return 2;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = await main();
}
