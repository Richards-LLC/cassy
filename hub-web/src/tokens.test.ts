import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const app = fileURLToPath(new URL("..", import.meta.url));
const read = (path: string) => readFileSync(join(app, path), "utf8");
const refs = (text: string) => [...text.matchAll(/(?:var\(\s*|getPropertyValue\(["'])(--[\w-]+)/g)].map((match) => match[1]);

function scopes() {
  return [...read("src/tokens.css").matchAll(/([^{}]+)\{([^{}]*)\}/g)].map((match) => ({
    selector: match[1].trim(),
    colorScheme: match[2].match(/color-scheme:\s*([^;]+);/)?.[1],
    properties: Object.fromEntries([...match[2].matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((decl) => [decl[1], decl[2].trim()])),
  }));
}

function assertResolved(properties: Record<string, string>, names: string[], chain: string[] = []) {
  for (const name of names) {
    if (!properties[name]) throw new Error(`Undefined token: ${name}`);
    if (chain.includes(name)) throw new Error(`Cyclic token: ${[...chain, name].join(" -> ")}`);
    assertResolved(properties, refs(properties[name]), [...chain, name]);
  }
}

it("regenerates the committed palette byte-for-byte from the vendored tokens", () => {
  const dir = mkdtempSync(join(tmpdir(), "commander-tokens-"));
  try {
    const output = join(dir, "tokens.css");
    execFileSync(process.execPath, ["scripts/generate-tokens.mjs", "--output", output], { cwd: app });
    expect(readFileSync(output, "utf8")).toBe(readFileSync(join(app, "src/tokens.css"), "utf8"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

it("defines every CSS and TypeScript token consumer in both schemes and dark wells", () => {
  const runtimeSources = readdirSync(join(app, "src"), { recursive: true })
    .filter((name): name is string => typeof name === "string" && name.endsWith(".ts") && !name.endsWith(".test.ts"));
  const consumed = refs([read("src/styles.css"), ...runtimeSources.map((path) => read(`src/${path}`))].join("\n"));
  expect(consumed.length).toBeGreaterThan(800);
  const blocks = scopes();
  const light = blocks.find((block) => block.selector === 'html[data-scheme="light"]')!.properties;
  const dark = blocks.find((block) => block.selector === 'html[data-scheme="dark"]')!.properties;
  expect(Object.keys(light).sort()).toEqual(Object.keys(dark).sort());
  for (const properties of [light, dark]) assertResolved(properties, [...consumed, ...Object.keys(properties)]);
  const well = blocks.find((block) => block.selector.includes(".terminal-mount"))!.properties;
  for (const properties of [light, dark]) assertResolved({ ...properties, ...well }, [...consumed, ...Object.keys(well)]);
  expect(light["--bg-root"]).toBe("#F7F4EE");
  expect(dark["--bg-root"]).toBe("#12141A");
  expect(well["--text-hi"]).toBe("#E9E6E0");
  expect(well["--text-mid"]).toBe("#A3A7B4");
  expect(blocks.filter((block) => block.selector.endsWith(":root"))).toHaveLength(2);
  expect(read("src/tokens.css")).toContain("@media (prefers-color-scheme: dark)");
  expect(read("src/styles.css")).not.toContain(":root {");
  for (const retired of ["--text-lo", "--fs-sm", "--space-5", "--space-10", "--state-info", "--fleet-card-min-width", "--connection-spin-duration"]) {
    expect(consumed).not.toContain(retired);
    expect(light).not.toHaveProperty(retired);
  }
});

it("forces native control schemes along with explicit palette overrides", () => {
  expect(scopes().find((block) => block.selector === 'html[data-scheme="light"]')!.colorScheme).toBe("light");
  expect(scopes().find((block) => block.selector === 'html[data-scheme="dark"]')!.colorScheme).toBe("dark");
});

it("names an undefined CSS consumer instead of silently accepting a fallback", () => {
  const properties = scopes().find((block) => block.selector === 'html[data-scheme="light"]')!.properties;
  expect(() => assertResolved(properties, refs(".example { color: var(--missing-colour); }"))).toThrow("Undefined token: --missing-colour");
});

it("fails generation with the missing source path named and leaves the output intact", () => {
  const dir = mkdtempSync(join(tmpdir(), "commander-tokens-"));
  try {
    const source = JSON.parse(read("../docs/design/design-tokens.json"));
    delete source.color.dark.ink;
    const input = join(dir, "source.json");
    const output = join(dir, "tokens.css");
    writeFileSync(input, JSON.stringify(source));
    writeFileSync(output, "previous output");
    const result = spawnSync(process.execPath, ["scripts/generate-tokens.mjs", "--source", input, "--output", output], { cwd: app, encoding: "utf8" });
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("color.dark.ink.$value");
    expect(readFileSync(output, "utf8")).toBe("previous output");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

it("detects committed-file drift without overwriting it before the test runs", () => {
  const dir = mkdtempSync(join(tmpdir(), "commander-tokens-"));
  try {
    const output = join(dir, "tokens.css");
    writeFileSync(output, `${read("src/tokens.css")}\n/* drift */\n`);
    const result = spawnSync(process.execPath, ["scripts/generate-tokens.mjs", "--check", "--output", output], { cwd: app, encoding: "utf8" });
    expect(result.status).toBe(1);
    expect(result.stderr).toContain("has drifted");
    expect(readFileSync(output, "utf8")).toContain("/* drift */");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

/** The flat `key: value` tree of a DESIGN.md frontmatter (the subset of YAML those files use). */
function frontmatter(path: string): Record<string, unknown> {
  const root: Record<string, unknown> = {};
  const stack: { indent: number; node: Record<string, unknown> }[] = [{ indent: -1, node: root }];
  for (const line of read(path).split(/^---$/m)[1].split("\n")) {
    const entry = line.match(/^( *)("[^"]+"|[\w.-]+): ?(.*)$/);
    if (!entry) continue;
    const [, space, rawKey, rawValue] = entry;
    while (stack.at(-1)!.indent >= space.length) stack.pop();
    const key = rawKey.replace(/^"|"$/g, "");
    if (rawValue === "") {
      const node: Record<string, unknown> = {};
      stack.at(-1)!.node[key] = node;
      stack.push({ indent: space.length, node });
    } else {
      stack.at(-1)!.node[key] = rawValue.startsWith("'") ? rawValue.slice(1, -1).replace(/''/g, "'") : rawValue.startsWith('"') ? JSON.parse(rawValue) : rawValue;
    }
  }
  return root;
}

/** What Commander paints in each scheme: the generated roles, re-coloured by glass.css's token blocks. */
function shipped(): Record<"light" | "dark", Record<string, string>> {
  const glass = read("src/glass.css").replace(/\/\*[\s\S]*?\*\//g, "");
  const glassBlock = (selector: string) => {
    const found = [...glass.matchAll(/([^{}]+)\{([^{}]*)\}/g)].find((match) => match[1].trim() === selector);
    if (!found) throw new Error(`glass.css has no ${selector} block`);
    return Object.fromEntries([...found[2].matchAll(/(--[\w-]+):\s*((?:"[^"]*"|[^;"])+);/g)].map((decl) => [decl[1], decl[2].replace(/\s+/g, " ").trim()]));
  };
  const house = (scheme: string) => scopes().find((block) => block.selector === `html[data-scheme="${scheme}"]`)!.properties;
  return {
    light: { ...house("light"), ...glassBlock(":root, html[data-scheme]"), ...glassBlock(':root, html[data-scheme="light"]') },
    dark: { ...house("dark"), ...glassBlock(":root, html[data-scheme]"), ...glassBlock('html[data-scheme="dark"]') },
  };
}

const same = (value: string) => value.replace(/\s+/g, "").toLowerCase();

it("documents the shipped Glass tokens in DESIGN.md frontmatter by name and value", () => {
  const { light, dark } = shipped();
  const design = frontmatter("DESIGN.md");
  const maps = design.maps as Record<string, string>;
  // pin: `source` is the doc's freshness anchor; it must name glass.css, the file whose values this test resolves.
  expect(design.source).toContain("hub-web/src/glass.css");
  let checked = 0;
  // Every colour names the CSS variable that carries it; the plain key is light, `-dark` is dark.
  for (const [role, value] of Object.entries(design.colors as Record<string, string>)) {
    const base = role.replace(/-dark$/, "");
    const name = maps[base];
    expect(name, `maps.${base}`).toMatch(/^--[\w-]+$/);
    expect(same(value), `colors.${role} (${name})`).toBe(same((role === base ? light : dark)[name]));
    if (role === base) expect(design.colors, `colors.${role}-dark`).toHaveProperty(`${role}-dark`);
    checked += 1;
  }
  // Typography levels list each property's variable; rounded and spacing steps name theirs.
  for (const [level, properties] of Object.entries(design.typography as Record<string, Record<string, string>>)) {
    const named = Object.fromEntries(maps[`typography.${level}`].split(", ").map((pair) => pair.split(" ")));
    expect(Object.keys(named).sort(), `typography.${level}`).toEqual(Object.keys(properties).sort());
    for (const [property, value] of Object.entries(properties)) expect(same(value), `typography.${level}.${property}`).toBe(same(light[named[property]]));
    checked += 1;
  }
  for (const group of ["rounded", "spacing"]) {
    for (const [step, value] of Object.entries(design[group] as Record<string, string>)) {
      expect(same(value), `${group}.${step}`).toBe(same(light[maps[`${group}.${step}`] ?? `--${step}`]));
      checked += 1;
    }
  }
  expect(checked).toBeGreaterThan(60);
});

it("gives the repo one brand that inherits the hub's tokens without a second palette", () => {
  const { light, dark } = shipped();
  const root = read("../DESIGN.md");
  const design = frontmatter("../DESIGN.md");
  expect(design.inherits).toBe("hub-web/DESIGN.md");
  expect(design).not.toHaveProperty("colors");
  const head = root.split(/^---$/m)[1];
  expect(head).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(|oklch\(/i);
  const named = [...head.matchAll(/--[a-z][\w-]*/g)].map((match) => match[0]);
  expect(named.length).toBeGreaterThan(10);
  for (const name of named) {
    expect(light, name).toHaveProperty(name);
    expect(dark, name).toHaveProperty(name);
  }
});
