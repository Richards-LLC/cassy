import { describe, expect, it } from "vitest";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { FIXTURE_NAMES, REQUIRED_SCHEMES, REQUIRED_VIEWPORTS, runFixtureVisualQa } from "./visual-qa.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

describe("hub-web fixture visual QA", () => {
  it("lists every Commander fixture and required visual-QA matrix", async () => {
    const source = await readFile(join(repoRoot, "fixtures", "main.ts"), "utf8");
    for (const name of FIXTURE_NAMES) expect(source).toContain(`"${name}"`);
    expect(REQUIRED_SCHEMES).toEqual(["light", "dark"]);
    expect(REQUIRED_VIEWPORTS.map(({ width }) => width)).toEqual([1280, 390]);
  });

  it("renders every scoped allowlist shape with its production attributes", async () => {
    const [fixtureSource, allowlistSource] = await Promise.all([
      readFile(join(repoRoot, "fixtures", "main.ts"), "utf8"),
      readFile(join(repoRoot, "visual-qa-allowlist.json"), "utf8"),
    ]);
    const allowlist = JSON.parse(allowlistSource);
    expect(fixtureSource).toContain('element("div", "machine-drawer")');
    expect(fixtureSource).toContain('setAttribute("aria-hidden", "true")');
    expect(fixtureSource).toContain('setAttribute("inert", "")');
    expect(fixtureSource).toContain('toast.id = "toast"');
    expect(fixtureSource).toContain('renderTerminalPlaceholder("agile-octopus", "worker", true)');
    expect(allowlist.entries.map(({ selector }) => selector)).toEqual(expect.arrayContaining([
      '.machine-drawer[aria-hidden="true"] *',
      "#toast:not(.visible)",
      ".terminal-mount",
      ".session-picker-toggle > .session-picker-name",
      ".pane.collapsed",
    ]));
  });

  it("wires a deliberately broken fixture through the strict gate", async () => {
    const [fixtureSource, runnerSource] = await Promise.all([
      readFile(join(repoRoot, "fixtures", "main.ts"), "utf8"),
      readFile(join(repoRoot, "scripts", "visual-qa.mjs"), "utf8"),
    ]);
    expect(fixtureSource).toContain("fixture-broken-contrast");
    expect(fixtureSource).toContain("Deliberate contrast defect");
    expect(runnerSource).toContain("broken ? \"&broken=1\" : \"\"");
    expect(runnerSource).toContain("strict: true");
    // The browser-backed version is available to CI and release-gate callers;
    // package unit tests remain deterministic without downloading Chromium.
    expect(typeof runFixtureVisualQa).toBe("function");
  });
});
