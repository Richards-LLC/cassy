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
    expect(fixtureSource).toContain("function appendOpenPairingDialog(view: PairingFixture): void");
    expect(fixtureSource).toContain("if (fixtureName.startsWith(\"pairing\")) appendOpenPairingDialog(fixtureName as PairingFixture);");
    expect(fixtureSource).toContain('if (!dialog.open) throw new Error("Pairing fixture dialog did not open");');
    expect(cssSource).toContain("container-type: inline-size;");
    expect(cssSource).toContain("font-size: clamp(44px, 10cqw, 76px);");
  });

  it("builds the composer and pairing dialog from the production markup builders (D11)", async () => {
    const [fixtureSource, conversationSource, mainSource] = await Promise.all([
      readFile(join(repoRoot, "fixtures", "main.ts"), "utf8"),
      readFile(join(repoRoot, "fixtures", "conversations.ts"), "utf8"),
      readFile(join(repoRoot, "src", "main.ts"), "utf8"),
    ]);
    // The app and the fixtures call the same builders.
    expect(mainSource).toContain("${composerMarkup(supervisor, operatorThreadMarkup(thread))}");
    expect(mainSource).toContain("return renderPairDialogMarkup({");
    expect(mainSource).toContain("applyMicState(mic, {");
    expect(conversationSource).toContain("slot.innerHTML = composerMarkup(supervisor);");
    expect(conversationSource).toContain("applyMicState(slot.querySelector<HTMLButtonElement>('#message-mic')!, fixtureMicState(state));");
    expect(fixtureSource).toContain("template.innerHTML = pairDialogMarkup({");
    expect(fixtureSource).toContain('dialog.querySelector<HTMLInputElement>("#pair-email")');
    // No hand-copied composer or pairing markup is left in the fixtures.
    for (const source of [fixtureSource, conversationSource]) {
      expect(source).not.toContain('id="message-text"');
      expect(source).not.toContain('id="message-send"');
      expect(source).not.toContain("Read machine, session, and pane state");
      expect(source).not.toContain('element("section", `pair-flow');
    }
  });

  it("registers the D11 states: long status, loading earlier, mic, live pairing field, catalog loading", async () => {
    const [conversationSource, row] = await Promise.all([
      readFile(join(repoRoot, "fixtures", "conversations.ts"), "utf8"),
      readFile(join(repoRoot, "fixtures", "hub-row-20812.txt"), "utf8"),
    ]);
    for (const name of [
      "conversation-long-status", "conversation-loading-earlier",
      "conversation-mic-idle", "conversation-mic-listening", "conversation-mic-unavailable",
      "pairing-email", "conversations-loading",
    ]) expect(FIXTURE_NAMES).toContain(name);
    // The long status is the real operator row, not a paraphrase.
    expect(Buffer.byteLength(row)).toBe(1079); // byte-identical to ~/.cas/artifacts/hub-row-20812.txt
    expect(row).toContain("WAITING ON YOU: (1)");
    expect(conversationSource).toContain("import LONG_STATUS from './hub-row-20812.txt?raw';");
    expect(conversationSource).toContain("reply(20812, null, LONG_STATUS.trim(), 'status', at(13, 25));");
    expect(conversationSource).toContain("hasEarlier: () => loadingEarlier, loadingEarlier: () => loadingEarlier,");
    expect(conversationSource).toContain("empty.textContent = conversationEmptyText(!loading, machines.length);");
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
