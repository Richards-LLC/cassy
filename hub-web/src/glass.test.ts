// Glass, Commander's only look (cas-675e). glass.css re-colours the generated
// roles; these checks pin its palette pairs and its accessibility media rules.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const read = (path: string) => readFileSync(fileURLToPath(new URL(path, import.meta.url)), "utf8");
const glass = read("./glass.css");
const main = read("./main.ts");

/** Custom properties of the first block whose selector list is exactly `selector`. */
function block(selector: string): Record<string, string> {
  for (const match of glass.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const found = match[1].replace(/\/\*[\s\S]*?\*\//g, "").trim().split("\n").at(-1)!.trim();
    if (found === selector) return Object.fromEntries([...match[2].matchAll(/(--[\w-]+):\s*((?:"[^"]*"|[^;"])+);/g)].map((decl) => [decl[1], decl[2].replace(/\s+/g, " ").trim()]));
  }
  throw new Error(`glass.css has no ${selector} block`);
}
const light = block(':root, html[data-scheme="light"]');
const dark = block('html[data-scheme="dark"]');
const darkMedia = block(':root:not([data-scheme="light"])');
/** House roles Glass does not override, from the generated tokens.css explicit-scheme blocks. */
function house(selector: string): Record<string, string> {
  const css = read("./tokens.css");
  const at = css.indexOf(`${selector} {`);
  if (at < 0) throw new Error(`tokens.css has no ${selector} block`);
  const body = css.slice(at, css.indexOf("}", at));
  return Object.fromEntries([...body.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((decl) => [decl[1], decl[2].trim()]));
}
const houseLight = house('html[data-scheme="light"]');
const houseDark = house('html[data-scheme="dark"]');

/** glass.css rules with comments stripped (media wrappers flattened): [selector list, declarations]. */
const glassRules = [...glass.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((m) => [m[1].trim().split("\n").at(-1)!.trim(), m[2]] as const);
const BUBBLE = ".turn.you .bub";
/**
 * The background glass.css paints on an operator bubble in a data-state
 * (null: no state), or undefined when no Glass rule reaches it and the base
 * applies. glass.css loads after styles.css at equal specificity, so the last
 * matching Glass rule wins.
 */
function bubbleBackground(state: string | null): string | undefined {
  let painted: string | undefined;
  for (const [list, body] of glassRules) {
    const background = body.match(/(?:^|;)\s*background:\s*([^;]+)/)?.[1]?.trim();
    if (!background) continue;
    for (const selector of list.split(/,(?![^(]*\))/).map((x) => x.trim())) {
      const at = selector.indexOf(BUBBLE);
      if (at < 0) continue;
      const tail = selector.slice(at + BUBBLE.length);
      if (/\s|\[data-(?!state)/.test(tail.replace(/\([^)]*\)/g, ""))) continue; // a descendant or another attribute
      const names = (tail.match(/data-state="([\w-]+)"/g) ?? []).map((x) => x.slice(12, -1));
      const negated = /^:not\(/.test(tail);
      const applies = names.length === 0 ? true : negated ? !(state && names.includes(state)) : Boolean(state && names.includes(state));
      if (applies) painted = background;
    }
  }
  return painted;
}

type Rgba = [number, number, number, number];
function parse(colour: string): Rgba {
  const hex = colour.match(/^#([0-9a-f]{6})$/i);
  if (hex) return [0, 2, 4].map((i) => Number.parseInt(hex[1].slice(i, i + 2), 16)).concat(1) as Rgba;
  const rgba = colour.match(/^rgba?\(([^)]+)\)$/);
  if (rgba) { const [r, g, b, a = "1"] = rgba[1].split(",").map((v) => v.trim()); return [Number(r), Number(g), Number(b), Number(a)]; }
  throw new Error(`Not a measurable colour: ${colour}`);
}
const over = (top: Rgba, under: Rgba): Rgba => [0, 1, 2].map((i) => top[i] * top[3] + under[i] * (1 - top[3])).concat(1) as Rgba;
const luminance = ([r, g, b]: Rgba) => [r, g, b].map((v) => v / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)).reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i], 0);
const ratio = (fg: Rgba, bg: Rgba) => { const [hi, lo] = [luminance(fg), luminance(bg)].sort((x, y) => y - x); return (hi + 0.05) / (lo + 0.05); };
const stops = (gradient: string) => [...gradient.matchAll(/#[0-9a-f]{6}|rgba?\([^)]+\)/gi)].map((m) => m[0]);

/** Every colour the aurora can show: each stop composited over each base-field colour. */
function auroraBackdrops(tokens: Record<string, string>): Rgba[] {
  const all = stops(tokens["--look-aurora-source"]);
  const field = stops(tokens["--look-aurora-source"].slice(tokens["--look-aurora-source"].lastIndexOf("linear-gradient"))).map(parse);
  return all.flatMap((stop) => field.map((base) => over(parse(stop), base)));
}
/** The pale middle the light thread's timestamps sit on: the inner blobs and the field. */
function readingField(tokens: Record<string, string>): Rgba[] {
  const layers = tokens["--look-aurora-source"].split(/,\s*(?=radial-gradient|linear-gradient)/);
  const field = stops(layers.at(-1)!).map(parse);
  return layers.slice(4).flatMap((layer) => stops(layer)).flatMap((stop) => field.map((base) => over(parse(stop), base)));
}

const FLOOR = 4.5;
describe.each([["light", light], ["dark", dark]] as const)("Glass %s", (scheme, t) => {
  const worst = (fg: string, backdrops: Rgba[], panel?: string) => Math.min(...backdrops.map((b) => ratio(over(parse(fg), panel ? over(parse(panel), b) : b), panel ? over(parse(panel), b) : b)));
  const aurora = auroraBackdrops(t);

  it("keeps text on every frosted panel at 4.5:1 over any aurora colour beneath it", () => {
    for (const panel of ["--look-glass", "--look-glass-strong", "--panel", "--sup-bg", "--sheet-bg"]) {
      for (const text of ["--ink", "--ink-mid", "--ink-soft"]) {
        expect(worst(t[text], aurora, t[panel]), `${scheme}: ${text} on ${panel}`).toBeGreaterThanOrEqual(FLOOR);
      }
    }
  });

  it("keeps opaque cards, chips and the selected row's waiting time readable", () => {
    const solid = (fg: string, bg: string) => ratio(parse(fg), parse(bg));
    for (const text of ["--ink", "--ink-mid"]) expect(solid(t[text], t["--bg-raised"]), `${scheme}: ${text} on --bg-raised`).toBeGreaterThanOrEqual(FLOOR);
    expect(solid(t["--pin-chip-fg"], t["--pin-chip-bg"])).toBeGreaterThanOrEqual(FLOOR);
    expect(solid(t["--tray-chip-fg"], t["--tray-chip-bg"])).toBeGreaterThanOrEqual(FLOOR);
    expect(solid(t["--accent-fg"], t["--accent"])).toBeGreaterThanOrEqual(FLOOR);
    expect(solid(t["--you-fg"], t["--you-bg"]), `${scheme}: compose on --you-bg`).toBeGreaterThanOrEqual(FLOOR);
    expect(solid(t["--you-bubble-fg"], t["--you-bubble-bg"])).toBeGreaterThanOrEqual(FLOOR);
    expect(worst(t["--color-action"], aurora, t["--look-glass"]), `${scheme}: action on glass`).toBeGreaterThanOrEqual(FLOOR);
  });

  it("keeps white on every stop of Send and the operator's bubble, and question ink on every ask stop", () => {
    for (const stop of stops(t["--look-send"])) expect(ratio(parse("#FFFFFF"), parse(stop)), `${scheme}: Send ${stop}`).toBeGreaterThanOrEqual(FLOOR);
    for (const stop of stops(t["--look-you"])) expect(ratio(parse(t["--you-bubble-fg"]), parse(stop)), `${scheme}: bubble ${stop}`).toBeGreaterThanOrEqual(FLOOR);
    for (const stop of stops(t["--look-ask"])) expect(ratio(parse(t["--ask-fg"]), parse(stop)), `${scheme}: ask ${stop}`).toBeGreaterThanOrEqual(FLOOR);
  });

  it.each([
    // [operator bubble state, the texts styles.css draws in it]
    ["sent", null, ["--you-bubble-fg"]],
    ["refused", "error", ["--ink", "--ink-mid", "--crit-bg"]],
    ["unconfirmed", "unconfirmed", ["--ink", "--ink-mid", "--warn-text"]],
  ] as const)("keeps every text of a %s operator bubble at 4.5:1 on the background Glass actually paints", (_label, state, texts) => {
    // QA cas-675e F03: an unscoped gradient rule reached refused and
    // unconfirmed bubbles and put --ink on violet at 1.2–2.8:1 in light.
    // Strict visual QA cannot see text over a background-image, so this
    // resolves the cascade from glass.css and measures every backdrop.
    const tokens = { ...(scheme === "light" ? houseLight : houseDark), ...t };
    const painted = bubbleBackground(state);
    const value = painted?.match(/^var\((--[\w-]+)\)$/)?.[1];
    const resolved = value ? tokens[value] : painted;
    let backdrops: Rgba[];
    if (resolved === undefined) backdrops = state ? aurora : [parse(tokens["--you-bubble-bg"])]; // base: unfilled record or solid bubble
    else if (/gradient\(/.test(resolved)) backdrops = stops(resolved).map(parse);
    else backdrops = aurora.map((b) => over(parse(resolved), b));
    for (const text of texts) {
      const fg = parse(tokens[text]);
      const low = Math.min(...backdrops.map((b) => ratio(over(fg, b), b)));
      expect(low, `${scheme}: ${text} on a ${_label} bubble painted ${painted ?? "by the base"}`).toBeGreaterThanOrEqual(FLOOR);
    }
  });

  it("keeps timestamps that sit straight on the aurora readable in its reading field", () => {
    // The vivid corners lie under the frosted panels; the middle column is the reading field.
    const field = readingField(t);
    expect(Math.min(...field.map((b) => ratio(parse(t["--ink-mid"]), b))), `${scheme}: ink-mid on the aurora`).toBeGreaterThanOrEqual(FLOOR);
  });
});

describe("Glass structure", () => {
  it("is the only look: imported after styles.css, with no look switch", () => {
    expect(main.indexOf('import "./glass.css";')).toBeGreaterThan(main.indexOf('import "./styles.css";'));
    expect(main).not.toContain("looks.css");
    expect(glass).not.toContain("data-look");
  });

  it("paints the aurora from its rendered image, never from live gradients", () => {
    // Live gradients behind see-through panels repainted on every update and
    // timed out HUB-J12 and HUB-J14; scripts/render-glass-aurora.mjs renders them.
    for (const tokens of [light, dark, darkMedia]) {
      expect(tokens["--look-aurora-source"]).toContain("radial-gradient(");
      expect(tokens["--look-aurora"]).toMatch(/^url\("data:image\/jpeg;base64,[A-Za-z0-9+/=]+"\) 0 0 \/ 100% 100% no-repeat var\(--bg-root\)$/);
    }
    expect(glass).not.toMatch(/background:[^;]*radial-gradient/);
  });

  it("defines one property set in light and dark, and the two dark blocks match", () => {
    expect(Object.keys(dark).sort()).toEqual(Object.keys(light).sort());
    expect(darkMedia).toEqual(dark);
  });

  it("keeps decoration out of forced colours, drops glass for more contrast, and never animates", () => {
    const forcedNone = glass.indexOf("@media (forced-colors: none) {");
    expect(forcedNone).toBeGreaterThan(-1);
    for (const decoration of ["background: var(--look-aurora)", "backdrop-filter: var(--look-blur);", "background: var(--look-send)", "background: var(--look-ask)"]) {
      expect(glass.indexOf(decoration), decoration).toBeGreaterThan(forcedNone);
    }
    const media = (query: string) => { const at = glass.indexOf(`@media ${query} {`); expect(at, query).toBeGreaterThan(-1); return glass.slice(at, glass.indexOf("\n}\n", at)); };
    const opaque = media("(prefers-contrast: more), (prefers-reduced-transparency: reduce), (forced-colors: active)");
    for (const rule of ["--look-blur: none;", "--look-glass: var(--bg-panel);", "--look-glass-strong: var(--bg-panel);", "--sup-bg: var(--bg-panel);"]) expect(opaque, rule).toContain(rule);
    const more = media("(prefers-contrast: more)");
    for (const rule of ["--look-aurora: var(--bg-root);", "--line-subtle: var(--line-strong);"]) expect(more, rule).toContain(rule);
    // Nothing moves behind glass: an animated backdrop re-blurs every frosted
    // panel each frame (30 fps instead of 59, and journeys timed out at 4 workers).
    const rules = glass.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const motion of ["animation", "@keyframes", "transition", "will-change"]) expect(rules, motion).not.toContain(motion);
    // Blur is for the four chrome panels and dialogs only, never per message:
    // a blur per bubble dropped a long thread's scroll from 56 fps to 43.
    expect(rules).not.toMatch(/\.bub[^{]*\{[^}]*backdrop-filter/);
  });

  it("measures text on every gradient Glass paints", () => {
    // Each gradient surface has a contrast pair above: Send and primaries
    // (white on --look-send), the operator bubble (--look-you, by state), the
    // question card (--ask-fg on --look-ask), its tray (opaque chips on
    // --look-ask-tray) and the aurora (reading field). A new one needs a pair.
    const rules = glass.replace(/\/\*[\s\S]*?\*\//g, "");
    const painted = new Set([...rules.matchAll(/background:\s*var\((--look-[\w-]+)\)/g)].map((m) => m[1]).filter((name) => /gradient\(|url\(/.test(light[name] ?? "")));
    expect([...painted].sort()).toEqual(["--look-ask", "--look-ask-tray", "--look-aurora", "--look-send", "--look-you"]);
  });

  it("never paints the violet gradient under a refused or unconfirmed message", () => {
    // QA cas-675e F03: an unscoped .turn.you .bub gradient overrode the base's
    // unfilled record (styles.css) and left near-black text on violet in light.
    const rules = glass.replace(/\/\*[\s\S]*?\*\//g, "");
    const youGradient = [...rules.matchAll(/([^{}]+)\{[^{}]*background:\s*var\(--look-you\)/g)].map((m) => m[1].trim());
    expect(youGradient.length).toBeGreaterThan(0);
    for (const selector of youGradient) expect(selector).toContain(':not(:is([data-state="error"], [data-state="unconfirmed"]))');
    expect(rules).toContain(':root .thread .turn.you .bub:is([data-state="error"], [data-state="unconfirmed"]) { background: var(--look-glass-strong); }');
  });
});
