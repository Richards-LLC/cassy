// The hub embeds fixed assets plus dist-root hashed JS/CSS chunks (cas-397a).
// Keep this pattern aligned with cas-cli/src/hub/chunk_assets.rs; an emitted
// file outside either set is a production serving failure.
import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Scratch production builds let factory lanes prove output without committing dist.
const dist = new URL(process.env.HUB_WEB_DIST_URL ?? "../dist/", import.meta.url);
const chunkName = /^chunk-[A-Za-z0-9_-]+-[A-Za-z0-9_-]{8}\.(?:js|css)$/;
const server = new URL("../../cas-cli/src/hub/server.rs", import.meta.url);

describe("production dist", () => {
  it("contains exactly the files the hub embeds", () => {
    const embedded = [...readFileSync(server, "utf8").matchAll(/include_bytes!\("\.\.\/\.\.\/\.\.\/hub-web\/dist\/([^"]+)"\)/g)].map((match) => match[1]);
    expect(embedded.length).toBeGreaterThan(0);
    // pin: Vite's eight-character base64url hash is the public serving contract.
    for (const [name, accepted] of [
      ["chunk-graph-Ab12_cd3.js", true], ["chunk-graph-Ab1-_cd3.css", true],
      ["chunk-graph.js", false], ["chunk-graph-Ab12_cd3.js.map", false],
      ["chunk-graph-Ab12_cd3.html", false], ["chunk-graph-Ab12_cd3.JS", false],
      ["dir/chunk-graph-Ab12_cd3.js", false], ["chunk-graph..-Ab12_cd3.js", false],
      ["chunk-graph-Ab12%00d3.js", false], ["chunk-graph-Ab12_cd3\0.js", false],
    ] as const) expect(chunkName.test(name), name).toBe(accepted);
    const built = readdirSync(dist).filter((name) => !name.startsWith("."));
    expect(built.filter((name) => !chunkName.test(name)).toSorted()).toEqual([...new Set(embedded)].toSorted());
    // pin: The embedded hub serves the entry stylesheet only at /commander/app.css.
    expect(readFileSync(new URL("index.html", dist), "utf8")).toContain('href="/commander/app.css"');
    const globalStyles = readFileSync(new URL("app.css", dist), "utf8");
    expect(globalStyles).toContain("body{");
    expect(globalStyles).not.toContain(".image-viewer{");
    const viewerStyles = built.find((name) => /^chunk-image-viewer-.*\.css$/.test(name));
    expect(viewerStyles, "the lazy viewer has its own hub-served stylesheet").toBeDefined();
    expect(readFileSync(new URL(viewerStyles!, dist), "utf8")).toContain(".image-viewer{");
  });

  it("carries no dynamic import the hub could not serve", () => {
    const app = readFileSync(new URL("app.js", dist), "utf8");
    // pin: Deployed literal chunk imports must resolve through the embedded routes.
    for (const match of app.matchAll(/import\(\s*["'`](\.\/?chunk-[^"'`]+)["'`]/g)) {
      const name = match[1].replace(/^\.\//, "");
      expect(chunkName.test(name), `unsupported dynamic chunk: ${name}`).toBe(true);
      expect(readdirSync(dist)).toContain(name);
    }
    expect(app).not.toMatch(/import\(\s*["']crypto["']\s*\)/);
  });

  it("ships the Pele permission notice and DOMPurify license comment in the renderer", () => {
    const name = readdirSync(dist).find((name) => /^chunk-rich-diagram-.*\.js$/.test(name))!;
    const renderer = readFileSync(new URL(name, dist), "utf8");
    expect(renderer).toContain(readFileSync(new URL("../licenses/pele-MIT.txt", import.meta.url), "utf8").trim());
    expect(renderer).toContain(readFileSync(new URL("../licenses/DOMPurify-LICENSE.txt", import.meta.url), "utf8").trim());
    expect(renderer).toContain("@license DOMPurify");
    expect(renderer).toContain("Cure53 and other contributors");
  });
});
