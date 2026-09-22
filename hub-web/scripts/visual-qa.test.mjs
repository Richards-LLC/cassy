import { describe, expect, it } from "vitest";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { FIXTURE_NAMES, REQUIRED_SCHEMES, REQUIRED_VIEWPORTS, runFixtureVisualQa } from "./visual-qa.mjs";
import { selectCachedPlaywright } from "../../scripts/visual-qa.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

describe("hub-web fixture visual QA", () => {
  it("lists every Commander fixture and required visual-QA matrix", async () => {
    const source = await readFile(join(repoRoot, "fixtures", "main.ts"), "utf8");
    for (const name of FIXTURE_NAMES) expect(source).toContain(`"${name}"`);
    expect(REQUIRED_SCHEMES).toEqual(["light", "dark"]);
    expect(REQUIRED_VIEWPORTS.map(({ width }) => width)).toEqual([1280, 390, 844]);
    expect(REQUIRED_VIEWPORTS.map(({ height }) => height)).toEqual([800, 844, 390]);
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
    expect(fixtureSource).toContain('toast.setAttribute("aria-hidden", "true")');
    expect(fixtureSource).toContain('renderTerminalPlaceholder("agile-octopus", "worker", true)');
    expect(allowlist.entries.map(({ selector }) => selector)).toEqual(expect.arrayContaining([
      ".terminal-mount",
      ".session-picker-toggle > .session-picker-name",
      ".pane.collapsed",
    ]));
  });

  it("keeps changed fixtures on the production connection and pairing surfaces", async () => {
    const [fixtureSource, mainSource, cssSource] = await Promise.all([
      readFile(join(repoRoot, "fixtures", "main.ts"), "utf8"),
      readFile(join(repoRoot, "src", "main.ts"), "utf8"),
      readFile(join(repoRoot, "src", "styles.css"), "utf8"),
    ]);
    expect(fixtureSource).toContain("renderConnectionSurfaceInto(card, \"bright-otter\"");
    expect(mainSource).toContain("renderConnectionSurfaceInto(placeholder, session, snapshot");
    expect(fixtureSource).toContain("K7MW-4H2Q");
    expect(fixtureSource).toContain("function appendOpenPairingDialog(cleanup: boolean): void");
    expect(fixtureSource).toContain("if (fixtureName === \"pairing-step-1\") appendOpenPairingDialog(false);");
    expect(fixtureSource).toContain("if (fixtureName === \"pairing-cleanup\") appendOpenPairingDialog(true);");
    expect(fixtureSource).toContain('if (!dialog.open) throw new Error("Pairing fixture dialog did not open");');
    expect(cssSource).toContain("container-type: inline-size;");
    expect(cssSource).toContain("font-size: clamp(44px, 10cqw, 76px);");
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

  it("picks the highest stable cached Playwright by version, never by directory order", async () => {
    const npxRoot = await mkdtemp(join(tmpdir(), "visual-qa-npx-"));
    try {
      // Directory names sort so that readdir order would end on the alpha.
      const cache = [["a-current", "1.63.0"], ["b-old", "1.56.1"], ["c-alpha", "1.64.0-alpha-1789764292000"], ["d-no-playwright", undefined]];
      for (const [dir, version] of cache) {
        const packageDir = join(npxRoot, dir, "node_modules", version ? "playwright" : "left-pad");
        await mkdir(packageDir, { recursive: true });
        await writeFile(join(packageDir, "package.json"), JSON.stringify({ name: version ? "playwright" : "left-pad", version: version ?? "1.0.0" }));
      }
      expect(selectCachedPlaywright(npxRoot)).toEqual({ packageDir: join(npxRoot, "a-current", "node_modules", "playwright"), version: "1.63.0" });
    } finally {
      await rm(npxRoot, { recursive: true, force: true });
    }
  });

  it("refuses a cache holding only prereleases instead of running on one", async () => {
    const npxRoot = await mkdtemp(join(tmpdir(), "visual-qa-npx-"));
    try {
      const packageDir = join(npxRoot, "only-alpha", "node_modules", "playwright");
      await mkdir(packageDir, { recursive: true });
      await writeFile(join(packageDir, "package.json"), JSON.stringify({ name: "playwright", version: "1.64.0-alpha-1789764292000" }));
      expect(selectCachedPlaywright(npxRoot)).toBeNull();
      expect(selectCachedPlaywright(join(npxRoot, "missing"))).toBeNull();
    } finally {
      await rm(npxRoot, { recursive: true, force: true });
    }
  });

  it("names the resolved Playwright on every run and keeps no directory-order fallback", async () => {
    const runner = await readFile(join(repoRoot, "..", "scripts", "visual-qa.mjs"), "utf8");
    expect(runner).toContain("console.log(`Playwright ${playwrightVersion} (${playwrightSource})`);");
    expect(runner).not.toContain("candidates.at(-1)");
  });
});
