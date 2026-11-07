import { describe, expect, it } from "vitest";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { FIXTURE_NAMES, REQUIRED_SCHEMES, REQUIRED_VIEWPORTS, runFixtureVisualQa } from "./visual-qa.mjs";
import { selectCachedPlaywright } from "../../scripts/visual-qa.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/** The names in fixtures/main.ts's own FIXTURE_NAMES list. */
function fixtureSiteNames(source) {
  const list = /export const FIXTURE_NAMES = \[([\s\S]*?)\];/.exec(source)?.[1];
  if (!list) throw new Error("fixtures/main.ts has no FIXTURE_NAMES list");
  return [...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
}

describe("hub-web fixture visual QA", () => {














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


});
