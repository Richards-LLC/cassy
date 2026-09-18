// Four distinct Pebble palettes. Each carries the full semantic set, and each
// is asserted against the same contrast floors the strict visual-qa probe uses
// (4.5:1 for normal text) BEFORE any CSS is written — so a palette cannot be
// committed on taste alone.
//
// Running this file emits schemes.css and exits non-zero if any required pair
// falls under the floor:
//   node docs/design/hub-messaging/round-3/schemes.mjs
//
// PAPER is primary: it is the hub token palette from hub-web/src/tokens.css and
// the only scheme that keeps a light and a dark variant (pebble.css owns those
// via prefers-color-scheme). The other three are pinned palettes applied with
// html[data-scheme="..."], which outranks both :root blocks in pebble.css.
//
// EMBER is dark-first: its hues were chosen for an emissive panel — a jade
// operator against warm black, luminous gold for the ask — not derived by
// inverting a light palette.

import { writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

export const SCHEMES = {
  graphite: {
    label: 'Graphite',
    note: 'cool slate, light-first — steel operator, a graphite fleet, amber ask',
    colorScheme: 'light',
    canvas: '#EEF0F3',
    panel: '#FBFCFD',
    sheet: '#FFFFFF',
    fold: '#DFE3E9',
    hair: '#D3D8E0',
    ink: '#171A20',
    inkMid: '#535963',
    inkSoft: '#434952',
    youBg: '#1F4E79',
    youFg: '#FFFFFF',
    askBg: '#E8B84B',
    askFg: '#171A20',
    askDeep: '#78550A',
    warnText: '#78550A',
    critBg: '#A82D22',
    critFg: '#FFFFFF',
    ok: '#1E6A4A',
    mFg: '#FFFFFF',
    machines: {
      atlas: { accent: '#3C5B86', sup: '#E3E8F0', soft: '#D5DCE8' },
      studio: { accent: '#1E6A4A', sup: '#E0EDE6', soft: '#CFE3D8' },
      bench: { accent: '#5A4A7C', sup: '#E7E4EF', soft: '#DAD5E8' },
    },
  },

  mono: {
    label: 'Mono',
    note: 'near-monochrome, light — the only saturated hue in the scheme is the one that wants you',
    colorScheme: 'light',
    canvas: '#F4F4F2',
    panel: '#FFFFFF',
    sheet: '#FFFFFF',
    fold: '#E4E4E1',
    hair: '#DAD9D5',
    ink: '#121316',
    inkMid: '#55565B',
    inkSoft: '#43444A',
    youBg: '#16181D',
    youFg: '#F7F7F5',
    askBg: '#F4913C',
    askFg: '#121316',
    askDeep: '#8A3F06',
    warnText: '#8A3F06',
    critBg: '#9E3405',
    critFg: '#FFFFFF',
    ok: '#2A2C31',
    mFg: '#FFFFFF',
    machines: {
      atlas: { accent: '#16181D', sup: '#E7E7E4', soft: '#DCDCD8' },
      studio: { accent: '#4C4F57', sup: '#EDEDEA', soft: '#E2E2DE' },
      bench: { accent: '#6A6D76', sup: '#F0F0EE', soft: '#E6E6E3' },
    },
  },

  ember: {
    label: 'Ember',
    note: 'dark-first, saturated — jade operator on warm black, luminous gold ask',
    colorScheme: 'dark',
    canvas: '#100E0C',
    panel: '#1A1714',
    sheet: '#241F1B',
    fold: '#2E2823',
    hair: '#302A24',
    ink: '#F3EDE4',
    inkMid: '#A99E90',
    inkSoft: '#C0B5A6',
    youBg: '#4FD6B2',
    youFg: '#100E0C',
    askBg: '#F7C948',
    askFg: '#100E0C',
    askDeep: '#6B4F0E',
    warnText: '#F7C948',
    critBg: '#FF8C7A',
    critFg: '#100E0C',
    ok: '#6FD99B',
    mFg: '#100E0C',
    machines: {
      atlas: { accent: '#F0A868', sup: '#2A2018', soft: '#231A14' },
      studio: { accent: '#6FD99B', sup: '#152219', soft: '#111C15' },
      bench: { accent: '#D8A0E8', sup: '#261B29', soft: '#1F1622' },
    },
  },

  slate: {
    label: 'Slate',
    note: 'cool dark — periwinkle operator, barely-raised supervisor wells, bright amber ask',
    colorScheme: 'dark',
    canvas: '#0A0D14',
    panel: '#151A25',
    sheet: '#1C2230',
    fold: '#242B3A',
    hair: '#232A38',
    ink: '#EDEBE5',
    inkMid: '#9AA2B4',
    inkSoft: '#B2B9C8',
    youBg: '#93A4FF',
    youFg: '#0A0D14',
    askBg: '#FFC857',
    askFg: '#0A0D14',
    askDeep: '#6E5214',
    warnText: '#FFC857',
    critBg: '#FF8D80',
    critFg: '#0A0D14',
    ok: '#63D9A4',
    mFg: '#0A0D14',
    machines: {
      atlas: { accent: '#7BD3E8', sup: '#16232B', soft: '#121D25' },
      studio: { accent: '#63D9A4', sup: '#16261F', soft: '#122019' },
      bench: { accent: '#C6A6F7', sup: '#241E33', soft: '#1D182B' },
    },
  },
};

// ---- contrast ------------------------------------------------------------

const srgb = (hex) => {
  const h = hex.replace('#', '');
  return [0, 2, 4].map((i) => Number.parseInt(h.slice(i, i + 2), 16) / 255);
};
const lum = (hex) => {
  const [r, g, b] = srgb(hex).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
export const ratio = (a, b) => {
  const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
};

const FLOOR = 4.5;

// Every text/background pair the surface actually renders.
export function pairsFor(s) {
  const out = [
    ['body text on canvas', s.ink, s.canvas],
    ['body text on panel', s.ink, s.panel],
    ['object text on its own surface', s.ink, s.sheet],
    ['muted text on canvas', s.inkMid, s.canvas],
    ['muted text on panel', s.inkMid, s.panel],
    ['attachment size line on its surface', s.inkMid, s.sheet],
    ['rail secondary on panel', s.inkSoft, s.panel],
    ['rail secondary on canvas', s.inkSoft, s.canvas],
    ['operator bubble text', s.youFg, s.youBg],
    ['ask body text on amber', s.askFg, s.askBg],
    ['blocker body text on crit', s.critFg, s.critBg],
    ['tray chip text on the chip', s.ink, s.panel],
    ['verified mark on its surface', s.ok, s.sheet],
    ['verified mark on panel', s.ok, s.panel],
    ['table flake cell on its surface', s.warnText, s.sheet],
    ['waiting time on panel', s.warnText, s.panel],
  ];
  for (const [name, m] of Object.entries(s.machines)) {
    out.push(
      [`supervisor bubble text · ${name}`, s.ink, m.sup],
      [`coalesced status · ${name}`, s.inkMid, m.sup],
      [`receipt tick · ${name}`, s.ok, m.sup],
      [`monogram on avatar · ${name}`, s.mFg, m.accent],
      [`selected-row headline · ${name}`, s.ink, m.soft],
      [`selected-row secondary · ${name}`, s.inkSoft, m.soft],
      [`selected-row muted · ${name}`, s.inkMid, m.soft],
      [`waiting time on selected row · ${name}`, s.warnText, m.soft],
    );
  }
  return out;
}

// ---- css emission --------------------------------------------------------

// pebble.css declares --lift* twice, once per OS colour scheme. A pinned
// palette must restate them or its shadows flip with the OS preference while
// its colours stay put — render.mjs catches exactly that by diffing the two
// screenshots. Dark palettes cast black; light palettes cast their own ink.
const shadows = (s) => {
  const dark = s.colorScheme === 'dark';
  const [r, g, b] = dark ? [0, 0, 0] : [0, 2, 4].map((i) => Number.parseInt(s.ink.slice(1 + i, 3 + i), 16));
  const a = dark ? [0.32, 0.34, 0.40, 0.46, 0.60, 0.70] : [0.05, 0.06, 0.08, 0.10, 0.22, 0.30];
  const c = (alpha) => `rgba(${r}, ${g}, ${b}, ${alpha.toFixed(2)})`;
  return `  --lift: 0 1px 2px ${c(a[0])}, 0 6px 18px ${c(a[1])};
  --lift-strong: 0 2px 4px ${c(a[2])}, 0 14px 34px ${c(a[3])};
  --lift-edge: 10px 0 30px -18px ${c(a[4])};
  --lift-head: 0 8px 20px -14px ${c(a[5])};`;
};

const block = (key, s) => {
  const m = s.machines;
  return `/* ${s.label} — ${s.note} */
html[data-scheme="${key}"] {
  color-scheme: ${s.colorScheme};
  --canvas: ${s.canvas};
  --panel: ${s.panel};
  --sheet-bg: ${s.sheet};
  --fold: ${s.fold};
  --hair: ${s.hair};
  --ink: ${s.ink};
  --ink-mid: ${s.inkMid};
  --ink-soft: ${s.inkSoft};
  --you-bg: ${s.youBg};
  --you-fg: ${s.youFg};
  --ask-bg: ${s.askBg};
  --ask-fg: ${s.askFg};
  --ask-deep: ${s.askDeep};
  --warn-text: ${s.warnText};
  --crit-bg: ${s.critBg};
  --crit-fg: ${s.critFg};
  --ok: ${s.ok};
  --m-fg: ${s.mFg};
${shadows(s)}
}
html[data-scheme="${key}"] .m-atlas { --accent: ${m.atlas.accent}; --accent-soft: ${m.atlas.soft}; --sup-bg: ${m.atlas.sup}; --sup-fg: ${s.ink}; }
html[data-scheme="${key}"] .m-studio { --accent: ${m.studio.accent}; --accent-soft: ${m.studio.soft}; --sup-bg: ${m.studio.sup}; --sup-fg: ${s.ink}; }
html[data-scheme="${key}"] .m-bench { --accent: ${m.bench.accent}; --accent-soft: ${m.bench.soft}; --sup-bg: ${m.bench.sup}; --sup-fg: ${s.ink}; }
`;
};

const HEADER = `/* Generated by schemes.mjs — do not edit by hand.
 *
 * Pinned Pebble palettes. html[data-scheme="..."] has specificity (0,1,1) and
 * therefore outranks both the :root and the prefers-color-scheme :root blocks
 * in pebble.css, so a pinned palette does not flip with the OS setting. The
 * per-machine rules are (0,2,1) and outrank pebble.css's .m-* accents.
 *
 * PAPER is the primary palette and is NOT listed here: it is the hub token
 * palette in pebble.css, and it is the only scheme that keeps both a light and
 * a dark variant.
 *
 * Every pair below was asserted at >= 4.5:1 by schemes.mjs before emission.
 */

`;

if (import.meta.url === `file://${process.argv[1]}`) {
  let failures = 0;
  let checked = 0;
  let worst = { name: '', value: Infinity, scheme: '' };
  for (const [key, s] of Object.entries(SCHEMES)) {
    for (const [name, fg, bg] of pairsFor(s)) {
      const r = ratio(fg, bg);
      checked += 1;
      if (r < worst.value) worst = { name, value: r, scheme: s.label };
      if (r < FLOOR) {
        failures += 1;
        console.error(`FAIL ${s.label} · ${name}: ${fg} on ${bg} = ${r.toFixed(2)}:1 (floor ${FLOOR})`);
      }
    }
  }
  console.log(`${checked} pairs checked across ${Object.keys(SCHEMES).length} pinned schemes, ${failures} under ${FLOOR}:1`);
  console.log(`tightest: ${worst.scheme} · ${worst.name} = ${worst.value.toFixed(2)}:1`);
  if (failures) process.exit(1);
  const css = HEADER + Object.entries(SCHEMES).map(([k, s]) => block(k, s)).join('\n');
  await writeFile(resolve(here, 'schemes.css'), css, 'utf8');
  console.log(`wrote schemes.css (${css.length} bytes)`);
}
