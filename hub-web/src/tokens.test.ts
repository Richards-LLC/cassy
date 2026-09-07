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

it("documents actual generated token values in DESIGN.md frontmatter", () => {
  const light = scopes().find((block) => block.selector === 'html[data-scheme="light"]')!.properties;
  const dark = scopes().find((block) => block.selector === 'html[data-scheme="dark"]')!.properties;
  const frontmatter = read("DESIGN.md").split("---")[1];
  expect(frontmatter).toContain("inherits: petrastella");
  const entries = [...frontmatter.matchAll(/^\s+[\w-]+: (".*")$/gm)].map((match) => JSON.parse(match[1]) as string);
  let checked = 0;
  for (const entry of entries) {
    for (const match of entry.matchAll(/(?:^|, )(--[\w-]+) /g)) {
      const name = match[1];
      expect(entry, name).toContain(`${name} ${light[name]}`);
      checked += 1;
    }
    if (entry.includes(" / ") && !entry.includes(" / 400") && !entry.includes(" / 600")) {
      const name = entry.split(" ")[0];
      expect(entry).toBe(`${name} ${light[name]} / ${dark[name]}`);
    }
  }
  expect(checked).toBeGreaterThan(40);
});
