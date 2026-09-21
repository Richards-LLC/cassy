import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { MACHINE_ACCENT_COUNT, fnv1a32, jumpConsistentHash, machineAccentClass, machineAccentIndex, machineMonogram } from "./machine-accent";

const tokens = readFileSync(fileURLToPath(new URL("./tokens.css", import.meta.url)), "utf8");

/** Every `selector { ... }` block of tokens.css with its custom properties. */
function scopes(): Array<{ selector: string; properties: Record<string, string> }> {
  return [...tokens.matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({
    selector: match[1].trim().split("\n").at(-1)!.trim(),
    properties: Object.fromEntries([...match[2].matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((decl) => [decl[1], decl[2].trim()])),
  }));
}
const scope = (selector: string) => {
  const found = scopes().find((block) => block.selector === selector);
  if (!found) throw new Error(`tokens.css has no ${selector} block`);
  return found.properties;
};

// WCAG 2.x relative luminance and contrast, the same arithmetic as
// docs/design/hub-messaging/round-3/schemes.mjs and scripts/visual-qa.mjs.
const channel = (hex: string, offset: number) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255;
const luminance = (hex: string) => [1, 3, 5].map((offset) => channel(hex, offset)).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)).reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i], 0);
export const contrast = (a: string, b: string) => {
  for (const hex of [a, b]) if (!/^#[0-9a-f]{6}$/i.test(hex)) throw new Error(`Not a measurable hex colour: ${hex}`);
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};
const FLOOR = 4.5;

/** The text/background pairs the Pebble surface renders, ported from round-3/schemes.mjs pairsFor(). */
function pairsFor(s: Record<string, string>, machine: Record<string, string>, name: string): Array<[string, string, string]> {
  return [
    ["body text on canvas", s["--ink"], s["--canvas"]],
    ["body text on panel", s["--ink"], s["--panel"]],
    ["object text on its own surface", s["--ink"], s["--sheet-bg"]],
    ["muted text on canvas", s["--ink-mid"], s["--canvas"]],
    ["muted text on panel", s["--ink-mid"], s["--panel"]],
    ["attachment size line on its surface", s["--ink-mid"], s["--sheet-bg"]],
    ["rail secondary on panel", s["--ink-soft"], s["--panel"]],
    ["rail secondary on canvas", s["--ink-soft"], s["--canvas"]],
    ["operator bubble text", s["--you-fg"], s["--you-bg"]],
    ["ask body text on amber", s["--ask-fg"], s["--ask-bg"]],
    ["blocker body text on crit", s["--crit-fg"], s["--crit-bg"]],
    ["tray chip text on the chip", s["--ink"], s["--panel"]],
    ["verified mark on its surface", s["--state-ok"], s["--sheet-bg"]],
    ["verified mark on panel", s["--state-ok"], s["--panel"]],
    ["table flake cell on its surface", s["--warn-text"], s["--sheet-bg"]],
    ["waiting time on panel", s["--warn-text"], s["--panel"]],
    [`supervisor bubble text · ${name}`, machine["--sup-fg"], machine["--sup-bg"]],
    [`coalesced status · ${name}`, s["--ink-mid"], machine["--sup-bg"]],
    [`receipt tick · ${name}`, s["--state-ok"], machine["--sup-bg"]],
    [`monogram and unread count on the accent · ${name}`, s["--accent-fg"], machine["--accent"]],
    [`selected-row headline · ${name}`, s["--ink"], machine["--accent-soft"]],
    [`selected-row secondary · ${name}`, s["--ink-soft"], machine["--accent-soft"]],
    [`selected-row muted · ${name}`, s["--ink-mid"], machine["--accent-soft"]],
    [`waiting time on selected row · ${name}`, s["--warn-text"], machine["--accent-soft"]],
  ];
}

describe("machine accent assignment", () => {
  it("is deterministic and stays inside the generated set", () => {
    for (const id of ["atlas-linux", "studio-mac", "bench-1", "", "🦀", "a-very-long-machine-identifier-0123456789"]) {
      const index = machineAccentIndex(id);
      expect(index).toBe(machineAccentIndex(id));
      expect(index).toBeGreaterThanOrEqual(0);
      expect(index).toBeLessThan(MACHINE_ACCENT_COUNT);
      expect(machineAccentClass(id)).toBe(`machine-accent-${index}`);
    }
    expect(fnv1a32("")).toBe(0x811c9dc5);
    expect(fnv1a32("a")).toBe(0xe40c292c);
  });

  it("pins the fixture machines to three distinct accents in design order", () => {
    expect([machineAccentIndex("atlas-linux"), machineAccentIndex("studio-mac"), machineAccentIndex("bench-1")]).toEqual([0, 1, 2]);
  });

  it("keeps every machine's accent when a fourth set is appended", () => {
    const ids = Array.from({ length: 2000 }, (_, index) => `machine-${index}`);
    let moved = 0;
    for (const id of ids) {
      const before = machineAccentIndex(id, MACHINE_ACCENT_COUNT);
      const after = machineAccentIndex(id, MACHINE_ACCENT_COUNT + 1);
      if (after !== before) { moved += 1; expect(after).toBe(MACHINE_ACCENT_COUNT); }
    }
    // Roughly 1/(N+1) of ids move, all of them into the new bucket only.
    expect(moved).toBeGreaterThan(ids.length / 8);
    expect(moved).toBeLessThan(ids.length / 2);
    expect(() => jumpConsistentHash(1, 0)).toThrow(RangeError);
  });

  it("takes the monogram from the first letter or digit", () => {
    expect(machineMonogram("Atlas · Linux")).toBe("A");
    expect(machineMonogram("  studio mac")).toBe("S");
    expect(machineMonogram("7th floor")).toBe("7");
    expect(machineMonogram("· ")).toBe("?");
  });
});

describe("Pebble accent contrast", () => {
  it("generates exactly MACHINE_ACCENT_COUNT accent sets for both schemes", () => {
    for (const scheme of ["light", "dark"]) {
      const sets = scopes().filter((block) => block.selector.startsWith(`html[data-scheme="${scheme}"] .machine-accent-`));
      expect(sets.map((block) => block.selector)).toEqual(Array.from({ length: MACHINE_ACCENT_COUNT }, (_, index) => `html[data-scheme="${scheme}"] .machine-accent-${index}`));
      for (const set of sets) expect(Object.keys(set.properties).sort()).toEqual(["--accent", "--accent-soft", "--sup-bg", "--sup-fg"]);
    }
    // The unscoped default is the first set, so a surface outside any machine still resolves.
    for (const scheme of ["light", "dark"]) {
      const root = scope(`html[data-scheme="${scheme}"]`);
      const first = scope(`html[data-scheme="${scheme}"] .machine-accent-0`);
      for (const name of Object.keys(first)) expect(root[name]).toBe(first[name]);
    }
  });

  it("measures every rendered pair at or above 4.5:1 in light and dark", () => {
    const measured: string[] = [];
    for (const scheme of ["light", "dark"]) {
      const root = scope(`html[data-scheme="${scheme}"]`);
      for (let index = 0; index < MACHINE_ACCENT_COUNT; index += 1) {
        const machine = scope(`html[data-scheme="${scheme}"] .machine-accent-${index}`);
        for (const [name, fg, bg] of pairsFor(root, machine, `accent ${index}`)) {
          const ratio = contrast(fg, bg);
          measured.push(`${scheme} · ${name}`);
          expect(ratio, `${scheme} · ${name}: ${fg} on ${bg} = ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(FLOOR);
        }
      }
    }
    expect(measured.length).toBe(2 * MACHINE_ACCENT_COUNT * 24);
    expect(contrast("#777777", "#FFFFFF")).toBeLessThan(FLOOR);
  });
});
