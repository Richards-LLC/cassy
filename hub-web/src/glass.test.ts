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

const styles = read("./styles.css");
type Rule = { selector: string; body: string; order: number };
const flatten = (css: string, from: number): Rule[] => [...css.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/([^{}]+)\{([^{}]*)\}/g)]
  .flatMap((m, index) => m[1].replace(/\s+/g, " ").trim().split(/,(?![^(]*\))/).map((selector) => ({ selector: selector.trim(), body: m[2], order: from + index })));
/** Drops @media blocks that do not apply to the default screen (forced colours, more contrast, reduced transparency, print). */
function defaultScreen(css: string): string {
  let out = css;
  for (let at = out.search(/@media[^{]*(forced-colors: active|prefers-contrast|prefers-reduced-transparency|print)[^{]*\{/); at >= 0; at = out.search(/@media[^{]*(forced-colors: active|prefers-contrast|prefers-reduced-transparency|print)[^{]*\{/)) {
    let depth = 0;
    let end = out.indexOf("{", at);
    for (; end < out.length; end += 1) {
      if (out[end] === "{") depth += 1;
      else if (out[end] === "}" && --depth === 0) break;
    }
    out = out.slice(0, at) + out.slice(end + 1);
  }
  return out;
}
// glass.css is imported after styles.css, so its rules win ties.
const cascadeRules = [...flatten(defaultScreen(styles), 0), ...flatten(defaultScreen(glass), 100_000)];
const declaration = (body: string, property: string) => body.match(new RegExp(`(?:^|;)\\s*${property}:\\s*([^;]+)`))?.[1]?.trim();
/** Selector specificity as [ids, classes/attributes/pseudo-classes, elements]; :where counts zero, :not/:is their most specific argument. */
function specificity(selector: string): [number, number, number] {
  let rest = selector.replace(/:where\((?:[^()]|\([^()]*\))*\)/g, "");
  const total: [number, number, number] = [0, 0, 0];
  rest = rest.replace(/:(?:not|is)\(((?:[^()]|\([^()]*\))*)\)/g, (_m, inner: string) => {
    const best = inner.split(/,(?![^(]*\))/).map((x) => specificity(x.trim())).sort((a, b) => b[0] - a[0] || b[1] - a[1] || b[2] - a[2])[0];
    for (const k of [0, 1, 2]) total[k] += best[k];
    return "";
  });
  total[0] += (rest.match(/#[\w-]+/g) ?? []).length;
  total[1] += (rest.match(/\.[\w-]+|\[[^\]]*\]|(?<!:):[\w-]+/g) ?? []).length;
  total[2] += (rest.match(/(?:^|[\s>+~])[a-z][\w-]*/g) ?? []).length + (rest.match(/::[\w-]+/g) ?? []).length;
  return total;
}
/** The winning value of `property` on `surface` at rest, by specificity and order across both stylesheets. */
function restValue(surfaces: string | readonly string[], property: string): string | undefined {
  const list = typeof surfaces === "string" ? [surfaces] : surfaces;
  const applies = cascadeRules.filter((rule) => {
    const surface = list.find((x) => rule.selector.includes(x));
    if (!surface) return false;
    const at = rule.selector.indexOf(surface);
    if (at < 0) return false;
    const tail = rule.selector.slice(at + surface.length);
    return !/^[\w-]/.test(tail) && !/[\s>+~:]/.test(tail.replace(/:not\((?:[^()]|\([^()]*\))*\)/g, "").replace(/\((?:[^()]|\([^()]*\))*\)/g, ""));
  }).map((rule) => ({ ...rule, weight: specificity(rule.selector) }))
    .sort((a, b) => a.weight[0] - b.weight[0] || a.weight[1] - b.weight[1] || a.weight[2] - b.weight[2] || a.order - b.order);
  return applies.map((rule) => declaration(rule.body, property)).filter(Boolean).at(-1);
}
const STATES = ["hover", "focus-visible", "focus", "active"] as const;
/**
 * The winning background, colour and filter for `surface` in each
 * interaction state that any rule names, resolved by specificity and order
 * across styles.css and glass.css. Rules that add an ancestor in `exclude`
 * (another surface's qualifier), that contain a `without` marker (another
 * state of the same surface) or that style a descendant are ignored.
 */
function interactionStates(surfaces: readonly string[], exclude: readonly string[] = [], without: readonly string[] = []) {
  const candidates = cascadeRules.flatMap((rule) => {
    if (without.some((x) => rule.selector.includes(x))) return [];
    // Bare `button:hover…` rules reach every button surface too (styles.css's generic hover wash).
    const surface = /^button(?=[:[]|$)/.test(rule.selector) && !/[\s>+~.#]/.test(rule.selector.replace(/\((?:[^()]|\([^()]*\))*\)/g, "")) ? "button" : surfaces.find((x) => rule.selector.includes(x));
    if (!surface) return [];
    const at = rule.selector.indexOf(surface);
    if (at < 0 || exclude.some((x) => rule.selector.slice(0, at).includes(x))) return [];
    const tail = rule.selector.slice(at + surface.length);
    if (/^[\w-]/.test(tail) || /[\s>+~]/.test(tail.replace(/\((?:[^()]|\([^()]*\))*\)/g, ""))) return [];
    const state = STATES.find((name) => new RegExp(`:${name}(?![\\w-])`).test(tail.replace(/:not\((?:[^()]|\([^()]*\))*\)/g, "")));
    return [{ ...rule, state, weight: specificity(rule.selector) }];
  });
  const named = new Set(candidates.flatMap((c) => (c.state ? [c.state] : [])));
  return [...named].map((state) => {
    const applies = candidates.filter((c) => !c.state || c.state === state).sort((a, b) => a.weight[0] - b.weight[0] || a.weight[1] - b.weight[1] || a.weight[2] - b.weight[2] || a.order - b.order);
    const win = (property: string, alt?: string) => applies.map((c) => declaration(c.body, property) ?? (alt ? declaration(c.body, alt) : undefined)).filter(Boolean).at(-1);
    return { state, background: win("background", "background-color"), color: win("color"), filter: win("filter"), shadow: win("box-shadow") };
  });
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

  it.each([
    // [surface (every one a button), its selectors in styles.css and glass.css, ancestors that mark a different surface, what lies beneath it, the colour it inherits]
    ["a question answer", [".obj.t-a .obj-foot button.chip", ".obj.t-a .obj-foot .chip"], [".pinned-ask"], "--look-ask-tray", null],
    ["a pinned question answer", [".pinned-ask .obj.t-a .obj-foot button.chip", ".pinned-ask .obj.t-a .obj-foot .chip"], [], "--look-ask-tray", null],
    ["Send", ["#message-send.send"], [], "glass", null],
    ["a primary action", [").primary"], [], "glass", null],
    ["the compose button", [".compose-fab"], [], "glass", null],
    ["Cancel in the operator's bubble", [".turn.you .bub .conversation-cancel", ".thread .conversation-edit"], [], "--look-you", "--you-bubble-fg"],
  ] as const)("keeps %s readable in every hover, focus and active state", (label, surface, exclude, under, inherited) => {
    // QA cas-205e: Glass's translucent --bg-hover replaced a question
    // answer's opaque pill on hover, leaving dark ink on the ember tray at
    // 1.95–2.64:1. Strict visual QA cannot see text over a gradient, so this
    // resolves each state's winning background, colour and filter from both
    // stylesheets and measures every stop beneath it.
    const tokens: Record<string, string> = { ...(scheme === "light" ? houseLight : houseDark), ...t };
    const resolve = (value: string): string => { const m = value.match(/^var\((--[\w-]+)(?:,\s*(.+))?\)$/); return m ? resolve(tokens[m[1]] ?? m[2]) : value; };
    const layers = (value: string): Rgba[] => (/gradient\(/.test(value) ? stops(value).map(parse) : [parse(value === "transparent" ? "rgba(0, 0, 0, 0)" : value)]);
    // "glass": a frosted panel over every aurora colour, where Send, primaries and the compose button sit.
    const underneath = under === "glass" ? aurora.map((b) => over(parse(t["--look-glass"]), b)) : layers(resolve(`var(${under})`));
    const states = interactionStates(surface, exclude);
    expect(states.length, `${label}: no interaction state found for ${surface}`).toBeGreaterThan(0);
    for (const { state, background, color, filter } of states) {
      const text = !color || color === "inherit" || color === "currentColor" ? (inherited ? `var(${inherited})` : color) : color;
      expect(text, `${label} :${state} has no text colour`).toBeTruthy();
      const ink = parse(resolve(text!));
      let painted = background ?? "transparent";
      const mix = painted.match(/^color-mix\(in srgb, currentColor (\d+)%, transparent\)$/);
      if (mix) painted = `rgba(${ink[0]}, ${ink[1]}, ${ink[2]}, ${Number(mix[1]) / 100})`;
      const brightness = Number(filter?.match(/brightness\(([\d.]+)\)/)?.[1] ?? 1);
      const lit = (c: Rgba): Rgba => [0, 1, 2].map((i) => Math.min(255, c[i] * brightness)).concat(1) as Rgba;
      const backdrops = layers(resolve(painted)).flatMap((top) => underneath.map((base) => lit(top[3] < 1 ? over(top, base) : top)));
      const low = Math.min(...backdrops.map((b) => ratio(lit(ink), b)));
      expect(low, `${scheme}: ${label} :${state} paints ${painted}${filter ? ` with ${filter}` : ""}, text ${text}`).toBeGreaterThanOrEqual(FLOOR);
    }
  });

  it("marks the open conversation with a fill and edge that hover cannot match", () => {
    // cas-6c8c: Glass hid the accent bar and set the open row to near-white
    // glass (about 1.05:1 against the list) while a hovered row stayed lilac,
    // so only the header said which conversation was open.
    const tokens: Record<string, string> = { ...(scheme === "light" ? houseLight : houseDark), ...t };
    const resolve = (value: string): string => { const m = value.match(/^var\((--[\w-]+)(?:,\s*(.+))?\)$/); return m ? resolve(tokens[m[1]] ?? m[2]) : value; };
    const list = aurora.map((b) => over(parse(t["--look-glass"]), b)); // the sidebar the rows sit on
    const open = cascadeRules.filter((r) => r.selector === ':root .conversation-row[aria-current="true"]').at(-1);
    expect(open, "glass.css styles the open row").toBeTruthy();
    const fill = declaration(open!.body, "background")!;
    const shadow = declaration(open!.body, "box-shadow") ?? "";
    // The edge: a full inset ring (never a left bar) in a colour at 3:1 or better against the list and the row's own fill.
    const ring = shadow.match(/inset 0 0 0 (\d+(?:\.\d+)?)px (var\([^)]+\)|#[0-9a-f]{6})/i);
    expect(ring, `${scheme}: the open row has an inset edge`).toBeTruthy();
    expect(Number(ring![1]), `${scheme}: open-row edge width (px)`).toBeGreaterThanOrEqual(2);
    const edge = parse(resolve(ring![2]));
    const filled = list.map((b) => over(parse(resolve(fill)), b));
    expect(Math.min(...list.map((b) => ratio(edge, b))), `${scheme}: open-row edge against the list`).toBeGreaterThanOrEqual(3);
    expect(Math.min(...filled.map((b) => ratio(edge, b))), `${scheme}: open-row edge against its fill`).toBeGreaterThanOrEqual(3);
    for (const text of ["--ink", "--ink-mid"]) expect(Math.min(...filled.map((b) => ratio(parse(tokens[text]), b))), `${scheme}: ${text} on the open row`).toBeGreaterThanOrEqual(FLOOR);
    // Hover on any other row: no edge, and a different fill.
    for (const { state, background, shadow: hoverShadow } of interactionStates([".conversation-row"], [], ["aria-current"]).filter((x) => x.state === "hover")) {
      expect(hoverShadow ?? "", `${scheme}: a :${state} row draws no open-row edge`).not.toMatch(/inset 0 0 0 \d/);
      expect(background, `${scheme}: a :${state} row is not filled like the open one`).not.toBe(fill);
    }
    // The open row keeps its fill and edge when hovered or focused.
    for (const { state, background, shadow: kept } of interactionStates(['.conversation-row[aria-current="true"]'], [])) {
      expect(background, `${scheme}: the open row :${state}`).toBe(fill);
      expect(kept, `${scheme}: the open row :${state}`).toBe(shadow);
    }
  });

  it("keeps the operator's delivery line at 4.5:1 on every violet stop", () => {
    // cas-205e: "Sending…" and "✓ Delivered" sat at opacity 0.85, 4.21:1 on the light stop.
    for (const meta of ["conversation-delivery", "conversation-delivered"]) {
      const opacity = Number(restValue([`.turn.you .bub .${meta}`, `.thread .${meta}`], "opacity") ?? 1);
      const ink = parse(t["--you-bubble-fg"]);
      for (const stop of stops(t["--look-you"]).map(parse)) {
        const shown = over([ink[0], ink[1], ink[2], opacity], stop);
        expect(ratio(shown, stop), `${scheme}: ${meta} at opacity ${opacity} on ${stop}`).toBeGreaterThanOrEqual(FLOOR);
      }
    }
  });

  it("gives every dialog's sticky actions a frosted bar, a gap above it and outlined buttons", () => {
    // cas-205e: an opaque bar fused with the installation sheet's last Revoke
    // at 390 and made Close read like an empty field.
    const tokens: Record<string, string> = { ...(scheme === "light" ? houseLight : houseDark), ...t };
    const resolve = (value: string): string => { const m = value.match(/^var\((--[\w-]+)(?:,\s*(.+))?\)$/); return m ? resolve(tokens[m[1]] ?? m[2]) : value; };
    expect(restValue("dialog .dialog-actions", "background")).toBe("var(--look-glass-bar)");
    // F02: a light frost, not a white slab over the sheet.
    expect(parse(t["--look-glass-bar"])[3]).toBeLessThanOrEqual(0.4);
    // The installations sheet's bar is its footer (outside the scroller): no frost, no extra height (QA F01).
    expect(restValue("dialog.installation-inventory > .dialog-actions", "background")).toBe("transparent");
    expect(restValue("dialog.installation-inventory > .dialog-actions", "margin-top")).toBe("0");
    expect(restValue("dialog .dialog-actions", "border-top")).toMatch(/^1px solid var\(--look-glass-line\)$/);
    expect(restValue("dialog .dialog-actions", "margin-top")).toBe("var(--space-3)");
    expect(Number.parseFloat(resolve("var(--space-3)"))).toBeGreaterThanOrEqual(8);
    const edge = restValue("dialog .dialog-actions button:not(.primary)", "border")!;
    expect(edge).toMatch(/solid var\(--line-strong\)/);
    const sheet = aurora.map((b) => over(parse(t["--look-glass-strong"]), b));
    const ring = parse(resolve("var(--line-strong)"));
    expect(Math.min(...sheet.map((b) => ratio(over(ring, b), b))), `${scheme}: Close's outline on the sheet`).toBeGreaterThanOrEqual(1.5);
    expect(Math.min(...sheet.map((b) => ratio(parse(t["--ink"]), b))), `${scheme}: Close's label on the sheet`).toBeGreaterThanOrEqual(FLOOR);
  });

  it("separates Browser installations from a destructive Remove in Paired machines", () => {
    // cas-205e: the two buttons touched and read as one control.
    const tokens: Record<string, string> = { ...(scheme === "light" ? houseLight : houseDark), ...t };
    const resolve = (value: string): string => { const m = value.match(/^var\((--[\w-]+)(?:,\s*(.+))?\)$/); return m ? resolve(tokens[m[1]] ?? m[2]) : value; };
    expect(Number.parseFloat(resolve(restValue(".paired-machine-installations", "margin-inline-end")!))).toBeGreaterThanOrEqual(8);
    const remove = ".paired-machine .paired-machine-remove";
    expect(restValue(remove, "color")).toBe("var(--crit-bg)");
    const sheet = aurora.map((b) => over(parse(t["--look-glass-strong"]), b));
    const fill = sheet.map((b) => over(parse(resolve(restValue(remove, "background")!)), b));
    expect(Math.min(...fill.map((b) => ratio(parse(resolve("var(--crit-bg)")), b))), `${scheme}: Remove's label`).toBeGreaterThanOrEqual(FLOOR);
  });

  it("shows the toast on a solid card", () => {
    // cas-205e: "Details copied" let the text beneath show through.
    const tokens: Record<string, string> = { ...(scheme === "light" ? houseLight : houseDark), ...t };
    const card = parse(tokens[restValue("#toast", "background")!.match(/^var\((--[\w-]+)\)$/)![1]]);
    expect(card[3], `${scheme}: toast background alpha`).toBe(1);
    expect(ratio(parse(t["--ink"]), card)).toBeGreaterThanOrEqual(FLOOR);
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
    for (const rule of ["--look-blur: none;", "--look-glass: var(--bg-panel);", "--look-glass-strong: var(--bg-panel);", "--look-glass-bar: var(--bg-panel);", "--sup-bg: var(--bg-panel);"]) expect(opaque, rule).toContain(rule);
    const more = media("(prefers-contrast: more)");
    for (const rule of ["--look-aurora: var(--bg-root);", "--line-subtle: var(--line-strong);"]) expect(more, rule).toContain(rule);
    // Nothing moves behind glass: an animated backdrop re-blurs every frosted
    // panel each frame (30 fps instead of 59, and journeys timed out at 4 workers).
    const rules = glass.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const motion of ["animation", "@keyframes", "will-change"]) expect(rules, motion).not.toContain(motion);
    // The one transition rule makes the toast appear at once (cas-205e); it adds no motion.
    const transitions = [...rules.matchAll(/([^{}]+)\{[^{}]*transition[^{}]*\}/g)].map((m) => m[0].replace(/\s+/g, " ").trim());
    expect(transitions).toEqual([":root #toast { background: var(--bg-panel); transition-duration: .2s, 0s; transition-property: transform, opacity; }"]);
    // Blur is for the four chrome panels and dialogs only, never per message:
    // a blur per bubble dropped a long thread's scroll from 56 fps to 43.
    expect(rules).not.toMatch(/\.bub[^{]*\{[^}]*backdrop-filter/);
  });

  it("measures text on every gradient Glass paints", () => {
    // Each gradient surface has a contrast pair above: Send and primaries at rest and hovered
    // (white on --look-send), the operator bubble (--look-you, by state), the
    // question card (--ask-fg on --look-ask), its tray (opaque chips on
    // --look-ask-tray) and the aurora (reading field). A new one needs a pair.
    const rules = glass.replace(/\/\*[\s\S]*?\*\//g, "");
    const painted = new Set([...rules.matchAll(/background:\s*var\((--look-[\w-]+)\)/g)].map((m) => m[1]).filter((name) => /gradient\(|url\(/.test(light[name] ?? "")));
    // Hovered Send and primaries (--look-send-hover) are measured by the interaction-state test.
    expect([...painted].sort()).toEqual(["--look-ask", "--look-ask-tray", "--look-aurora", "--look-send", "--look-send-hover", "--look-you"]);
  });

  it("flattens the operator's bubble under more contrast and makes phone sheets opaque", () => {
    // cas-205e: under prefers-contrast: more the gradient left the delivery
    // line at 4.44:1; full-screen phone sheets let the page ghost through.
    const at = glass.indexOf("@media (forced-colors: none) and (prefers-contrast: more) {");
    expect(at, "a more-contrast block after the Glass surfaces").toBeGreaterThan(glass.lastIndexOf("background: var(--look-you)"));
    expect(glass.slice(at)).toContain(':root .thread .turn.you .bub:not(:is([data-state="error"], [data-state="unconfirmed"])) { background: var(--you-bubble-bg); box-shadow: none; }');
    for (const tokens of [light, dark]) expect(ratio(parse(tokens["--you-bubble-fg"]), parse(tokens["--you-bubble-bg"]))).toBeGreaterThanOrEqual(FLOOR);
    const rules = glass.replace(/\/\*[\s\S]*?\*\//g, "");
    expect(rules).toMatch(/:root dialog\.launch-sheet,\s*:root \.conversation-shell\.attention-sheet-open > \.conversation-context \{ background: var\(--bg-panel\); -webkit-backdrop-filter: none; backdrop-filter: none;/);
    for (const tokens of [light, dark]) expect(parse(tokens["--bg-panel"])[3]).toBe(1);
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
