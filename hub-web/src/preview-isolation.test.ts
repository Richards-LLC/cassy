import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

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
});
