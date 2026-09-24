import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { MACHINE_ACCENT_BASE, MACHINE_ACCENT_COUNT, MACHINE_ACCENT_STORAGE_KEY, assignMachineAccents, fnv1a32, jumpConsistentHash, machineAccentClass, machineAccentIndex, machineInitials, machineMonogram, setMachineAccentFleet, storageAccentStore } from "./machine-accent";

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

  it("gives a paired fleet distinct accents while any is free (journey F16)", () => {
    // The journey world's two machines hash to the same accent on their own.
    expect(machineAccentIndex("atlas")).toBe(machineAccentIndex("studio"));
    const pair = assignMachineAccents(["studio", "atlas"]);
    expect(pair.get("atlas")).toBe(machineAccentIndex("atlas"));
    expect(pair.get("studio")).not.toBe(pair.get("atlas"));
    // Order-independent: the same fleet gets the same colours on every device.
    expect(assignMachineAccents(["atlas", "studio"])).toEqual(pair);
    // A fleet no larger than the set never repeats; a larger one spreads evenly.
    for (let size = 1; size <= 3 * MACHINE_ACCENT_COUNT; size += 1) {
      const fleet = assignMachineAccents(Array.from({ length: size }, (_, index) => `machine-${index}`));
      const counts = new Array<number>(MACHINE_ACCENT_COUNT).fill(0);
      for (const index of fleet.values()) counts[index] += 1;
      expect(Math.max(...counts) - Math.min(...counts), `fleet of ${size}: ${counts}`).toBeLessThanOrEqual(1);
    }
    // A single machine keeps its hashed accent.
    expect(assignMachineAccents(["bench-1"]).get("bench-1")).toBe(machineAccentIndex("bench-1"));
  });

  it("colours rows from the registered fleet, falling back to the hash for strangers", () => {
    setMachineAccentFleet(["atlas", "studio"]);
    try {
      expect(machineAccentClass("atlas")).not.toBe(machineAccentClass("studio"));
      expect(machineAccentClass("elsewhere")).toBe(`machine-accent-${machineAccentIndex("elsewhere")}`);
    } finally {
      setMachineAccentFleet([]);
    }
  });

  it("keeps a machine's stored accent when another machine pairs or leaves (cas-50a7)", () => {
    // Without storage, pairing "alpha" (sorts first, hashes to atlas's accent) re-colours both.
    const before = assignMachineAccents(["atlas", "studio"]);
    const naive = assignMachineAccents(["alpha", "atlas", "studio"]);
    expect([naive.get("atlas"), naive.get("studio")]).not.toEqual([before.get("atlas"), before.get("studio")]);
    // With the accents stored when atlas and studio paired, they keep them.
    const kept = assignMachineAccents(["alpha", "atlas", "studio"], MACHINE_ACCENT_COUNT, before);
    expect(kept.get("atlas")).toBe(before.get("atlas"));
    expect(kept.get("studio")).toBe(before.get("studio"));
    expect(new Set(kept.values()).size).toBe(3);
    // Removing one leaves the other alone.
    const after = assignMachineAccents(["studio"], MACHINE_ACCENT_COUNT, kept);
    expect(after.get("studio")).toBe(before.get("studio"));
    // A stored accent another member now holds (removed, colour reused, re-paired) is reassigned, not duplicated.
    const clash = assignMachineAccents(["atlas", "zed"], MACHINE_ACCENT_COUNT, new Map([["atlas", 1], ["zed", 1]]));
    expect(clash.get("atlas")).toBe(1);
    expect(clash.get("zed")).not.toBe(1);
    // Out-of-range or non-integer entries are ignored.
    expect(assignMachineAccents(["bench-1"], MACHINE_ACCENT_COUNT, new Map([["bench-1", 99]])).get("bench-1")).toBe(machineAccentIndex("bench-1"));
  });

  it("gives a fleet of up to three exactly the colours it had, and a fourth and fifth machine the appended accents", () => {
    // The pre-cas-50a7 rule over the base set, reproduced for comparison.
    const legacy = (ids: string[]) => {
      const uses = new Array<number>(MACHINE_ACCENT_BASE).fill(0); const out = new Map<string, number>();
      for (const id of [...new Set(ids)].sort()) { const least = Math.min(...uses); let index = machineAccentIndex(id); while (uses[index] !== least) index = (index + 1) % MACHINE_ACCENT_BASE; uses[index] += 1; out.set(id, index); }
      return out;
    };
    for (let trial = 0; trial < 200; trial += 1) {
      const ids = Array.from({ length: 1 + (trial % MACHINE_ACCENT_BASE) }, (_, index) => `m-${trial}-${index}`);
      expect(assignMachineAccents(ids)).toEqual(legacy(ids));
    }
    const five = assignMachineAccents(["atlas-linux", "studio-mac", "bench-1", "forge", "vega"]);
    expect(new Set(five.values()).size).toBe(5);
    expect([...five.values()].filter((index) => index >= MACHINE_ACCENT_BASE).sort()).toEqual([3, 4]);
  });

  it("records each machine's accent in storage the first time it is seen", () => {
    const data = new Map<string, string>();
    const storage = { getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => { data.set(key, value); } };
    try {
      setMachineAccentFleet(["atlas", "studio"], storageAccentStore(storage));
      const first = JSON.parse(data.get(MACHINE_ACCENT_STORAGE_KEY)!);
      const atlas = machineAccentClass("atlas"); const studio = machineAccentClass("studio");
      // A fresh page (new store over the same storage) pairs "alpha": the others keep their colours.
      setMachineAccentFleet(["alpha", "atlas", "studio"], storageAccentStore(storage));
      expect(machineAccentClass("atlas")).toBe(atlas);
      expect(machineAccentClass("studio")).toBe(studio);
      expect(machineAccentClass("alpha")).not.toBe(atlas);
      expect(machineAccentClass("alpha")).not.toBe(studio);
      expect(JSON.parse(data.get(MACHINE_ACCENT_STORAGE_KEY)!)).toMatchObject(first);
      // Unreadable storage degrades to hashing, never throws.
      data.set(MACHINE_ACCENT_STORAGE_KEY, "not json");
      expect(() => setMachineAccentFleet(["atlas"], storageAccentStore(storage))).not.toThrow();
      expect(() => setMachineAccentFleet(["atlas"], storageAccentStore({ getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); } }))).not.toThrow();
    } finally {
      setMachineAccentFleet([]);
    }
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
      for (const set of sets) expect(Object.keys(set.properties).sort()).toEqual(["--accent", "--accent-soft", "--lift-sup", "--sup-bg", "--sup-fg"]);
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

/** Composite a token over an opaque hex surface: hex passes through, rgba() is alpha-blended. */
const flatten = (value: string, surface: string) => {
  const rgba = value.match(/^rgba\(\s*(\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\s*\)$/);
  if (!rgba) return value;
  const alpha = Number(rgba[4]);
  return "#" + [1, 2, 3].map((i) => Math.round(Number(rgba[i]) * alpha + channel(surface, 2 * i - 1) * 255 * (1 - alpha)).toString(16).padStart(2, "0")).join("").toUpperCase();
};

describe("selected and active states carry a >= 3:1 edge cue (WCAG 1.4.11, cas-08b4)", () => {
  const css = readFileSync(fileURLToPath(new URL("./styles.css", import.meta.url)), "utf8");

  it("draws an accent edge bar on the selected conversation row and the active machine", () => {
    expect(css).toContain('.conversation-row[aria-current="true"]::before { content: ""; position: absolute;');
    expect(css).toMatch(/\.conversation-row\[aria-current="true"\]::before \{[^}]*background: var\(--accent\);/);
    expect(css).toMatch(/\.machine-icon\.active::before \{[^}]*background: var\(--accent\);/);
    expect(css).toContain(".conversation-row { position: relative;");
  });

  it("measures each bar at or above 3:1 against the fill it sits on, in light and dark", () => {
    for (const scheme of ["light", "dark"]) {
      const root = scope(`html[data-scheme="${scheme}"]`);
      for (let index = 0; index < MACHINE_ACCENT_COUNT; index += 1) {
        const machine = scope(`html[data-scheme="${scheme}"] .machine-accent-${index}`);
        const ratio = contrast(machine["--accent"], machine["--accent-soft"]);
        expect(ratio, `${scheme} · selected-row bar · accent ${index}: ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(3);
      }
      const active = flatten(root["--bg-active"], root["--bg-panel"]);
      const ratio = contrast(root["--accent"], active);
      expect(ratio, `${scheme} · active machine bar: ${root["--accent"]} on ${active} = ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(3);
    }
  });
});

/** A token that resolves to `transparent` shows the surface it sits on. */
const over = (value: string, surface: string) => (value === "transparent" ? surface : value);

describe("dark tints instead of floods (P9, cas-9616)", () => {
  it("keeps every tint, bubble and tray chip pair at or above 4.5:1 text and 3:1 marks in both schemes", () => {
    for (const scheme of ["light", "dark"]) {
      const s = scope(`html[data-scheme="${scheme}"]`);
      const trayChip = over(s["--tray-chip-bg"], s["--ask-tint-deep"]);
      const text: Array<[string, string, string]> = [
        ["in-thread ask ink on its tint", s["--ask-tint-fg"], s["--ask-tint"]],
        ["in-thread tray chip text", s["--tray-chip-fg"], trayChip],
        ["sent tick on an in-thread tray chip", s["--state-ok"], trayChip],
        ["pinned ask ink on full gold", s["--ask-fg"], s["--ask-bg"]],
        ["pinned tray chip text", s["--pin-chip-fg"], s["--pin-chip-bg"]],
        ["in-thread blocker ink on its tint", s["--crit-tint-fg"], s["--crit-tint"]],
        ["operator bubble text", s["--you-bubble-fg"], s["--you-bubble-bg"]],
      ];
      for (const [name, fg, bg] of text) {
        const ratio = contrast(fg, bg);
        expect(ratio, `${scheme} · ${name}: ${fg} on ${bg} = ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(FLOOR);
      }
      const marks: Array<[string, string, string]> = [
        ["pinned tray chip against the pinned tray", s["--pin-chip-bg"], s["--ask-deep"]],
        ["chip focus ring on the pinned tray", s["--tray-focus"], s["--ask-deep"]],
        ["chip focus ring on an in-thread tray", s["--tray-focus"], s["--ask-tint-deep"]],
      ];
      if (scheme === "dark") marks.push(
        ["ask edge against its tint", s["--ask-edge"], s["--ask-tint"]],
        ["ask edge against the canvas", s["--ask-edge"], s["--canvas"]],
        ["tray chip outline against the tray", s["--tray-chip-line"], s["--ask-tint-deep"]],
        ["blocker edge against its tint", s["--crit-edge"], s["--crit-tint"]],
        ["blocker edge against the canvas", s["--crit-edge"], s["--canvas"]],
        ["collapsed-ask edge (design polish 1) on the supervisor pebble", s["--ask-bg"], s["--sup-bg"]],
      );
      for (const [name, fg, bg] of marks) {
        const ratio = contrast(fg, bg);
        expect(ratio, `${scheme} · ${name}: ${fg} on ${bg} = ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(3);
      }
    }
  });

  it("tints the dark in-thread objects and keeps light exactly as it was", () => {
    const light = scope('html[data-scheme="light"]');
    const dark = scope('html[data-scheme="dark"]');
    // Light: the in-thread object is the pinned object, so nothing on paper moves.
    expect([light["--ask-tint"], light["--ask-tint-fg"], light["--ask-tint-deep"]]).toEqual([light["--ask-bg"], light["--ask-fg"], light["--ask-deep"]]);
    expect([light["--crit-tint"], light["--crit-tint-fg"]]).toEqual([light["--crit-bg"], light["--crit-fg"]]);
    expect([light["--you-bubble-bg"], light["--you-bubble-fg"]]).toEqual([light["--you-bg"], light["--you-fg"]]);
    expect([light["--ask-edge"], light["--crit-edge"], light["--tray-chip-line"]]).toEqual(["transparent", "transparent", "transparent"]);
    // Dark: tints darker than the flood they replace; the pinned ask keeps the gold.
    expect(dark["--ask-tint"]).toBe("#3A3020");
    expect(dark["--crit-tint"]).toBe("#3A1F1E");
    expect(dark["--ask-edge"]).toBe(dark["--ask-bg"]);
    expect(dark["--crit-edge"]).toBe(dark["--crit-bg"]);
    // The operator's bubble is quieter than Send: Send keeps the bright accent.
    expect(dark["--you-bubble-bg"]).toBe("#3A46B0");
    expect(dark["--you-bg"]).toBe(dark["--accent"]);
    for (const [name, fill] of [["ask", dark["--ask-tint"]], ["blocker", dark["--crit-tint"]], ["operator bubble", dark["--you-bubble-bg"]]]) {
      expect(contrast(fill, dark["--canvas"]), `${name} fill must sit far below Send against the canvas`).toBeLessThan(contrast(dark["--accent"], dark["--canvas"]) / 3);
    }
  });

  it("derives the raised and hover steps from the page, not the panel", () => {
    for (const scheme of ["light", "dark"]) {
      const s = scope(`html[data-scheme="${scheme}"]`);
      expect(s["--bg-raised"]).toBe("color-mix(in srgb, var(--bg-root) 92%, var(--text-hi))");
      expect(s["--bg-hover"]).toBe("color-mix(in srgb, var(--bg-root) 88%, var(--text-hi))");
      expect(s["--bg-root"]).toBe(s["--canvas"]);
    }
  });
});

describe("one thread surface and one selection colour (journey F11)", () => {
  const css = readFileSync(fileURLToPath(new URL("./styles.css", import.meta.url)), "utf8");

  it("keeps the main-action button styles off the primary terminal pane", () => {
    // pane-layout marks the primary pane .primary; an unscoped .primary:hover
    // brightened the whole thread from cream to white under the pointer.
    expect(css).not.toMatch(/^\.primary[\s:{,]/m);
    expect(css).toContain(":where(button, a).primary {");
    expect(css).toMatch(/:where\(button, a\)\.primary:hover:not\(:disabled\)[^{]*\{[^}]*filter: brightness\(1\.08\)/);
  });

  it("keeps the open row's accent tint under the pointer", () => {
    expect(css).toMatch(/\.conversation-row\[aria-current="true"\]:is\(:hover, :focus-visible\):not\(:disabled\) \{ background: var\(--accent-soft\); \}/);
  });

  it("outlines supervisor bubbles with an accent hairline in every accent and scheme", () => {
    expect(css).toMatch(/\.thread \.bub \{[^}]*box-shadow: var\(--lift-sup\);/);
    for (const scheme of ["light", "dark"]) {
      for (let index = 0; index < MACHINE_ACCENT_COUNT; index += 1) {
        const machine = scope(`html[data-scheme="${scheme}"] .machine-accent-${index}`);
        expect(machine["--lift-sup"], `${scheme} · accent ${index}`).toBe(`${scope(`html[data-scheme="${scheme}"]`)["--lift"]}, inset 0 0 0 1px color-mix(in srgb, ${machine["--accent"]} 30%, transparent)`);
      }
    }
  });
});

describe("machine rail initials (3.30.0 journey F3)", () => {
  it("takes letters from the machine's own name, never a separator", () => {
    expect(machineInitials("Atlas · Linux")).toBe("AT");
    expect(machineInitials("Studio Mac · macOS")).toBe("SM");
    expect(machineInitials("Alpha · Linux")).toBe("AL");
    expect(machineInitials("build-box-2 · Linux")).toBe("BB");
    expect(machineInitials("Atlas")).toBe("AT");
    expect(machineInitials("  ")).toBe("?");
  });

  it("falls back to the whole label when the name part holds no letters", () => {
    expect(machineInitials("· Linux")).toBe("LI");
    expect(machineInitials("— · —")).toBe("?");
  });
});

