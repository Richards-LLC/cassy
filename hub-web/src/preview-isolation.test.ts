import { mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { describe, expect, it } from "vitest";
import { build } from "vite";

// cas-675e: the clickable preview (hub-web/preview) runs the production bundle
// against an in-page fixture hub. That shim, and the journey HubDouble it
// wraps, must never reach the shipped bundle in hub-web/dist.
const dist = join(__dirname, "..", "dist");
const MARKERS = [
  "PreviewPage", "PreviewSocket", "PreviewRoute", "installTransport", "preview guard",
  "HubDouble", "hub double:", "__journeyOutage", "__journeyMachineEvent", "journey-double", "journey-only",
  "/commander/preview/", "preview/shim.js",
];

describe("preview isolation", () => {
  const files = readdirSync(dist).filter((name) => /\.(js|css|html)$/.test(name));

  it("ships a bundle", () => {
    expect(files).toContain("app.js");
    expect(files).toContain("index.html");
  });

  for (const file of files) {
    it(`${file} carries no preview or fixture-hub code`, () => {
      const text = readFileSync(join(dist, file), "utf8");
      for (const marker of MARKERS) expect(text.includes(marker), `${file} contains ${marker}`).toBe(false);
    });
  }

  it("the production entry never imports the preview", () => {
    const main = readFileSync(join(__dirname, "main.ts"), "utf8");
    expect(main).not.toMatch(/from ["'][^"']*preview|import\(["'][^"']*preview/);
  });

  it("builds the clickable preview without test runners or Node modules", async () => {
    const modules = new Set<string>();
    const unrelated = mkdtempSync(join(realpathSync(tmpdir()), "cas-preview-resolve-"));
    try {
      writeFileSync(join(unrelated, "main.ts"), 'export { runtime } from "./hub-double-runtime";');
      writeFileSync(join(unrelated, "hub-double-runtime.ts"), 'export const runtime = "unrelated runtime";');
      await build({
        configFile: join(__dirname, "..", "vite.preview.config.ts"),
        logLevel: "silent",
        build: {
          write: false,
          lib: {
            entry: { preview: join(__dirname, "../preview/main.ts"), unrelated: join(unrelated, "main.ts") },
            fileName: (_format, name) => `${name}.js`,
          },
        },
        plugins: [{
          name: "preview-module-audit",
          generateBundle() {
            for (const id of this.getModuleIds()) modules.add(id);
          },
        }],
      });
      expect([...modules].filter(id => /playwright|node:|__vite-browser-external/.test(id))).toEqual([]);
      expect([...modules].some(id => id.endsWith("/preview/main.ts"))).toBe(true);
      expect(modules.has(join(unrelated, "hub-double-runtime.ts"))).toBe(true);
    } finally {
      rmSync(unrelated, { recursive: true, force: true });
    }
  });
});
