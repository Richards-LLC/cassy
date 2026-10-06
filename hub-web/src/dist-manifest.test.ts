// cas-9b7d: the hub serves Commander from a fixed list of embedded files
// (cas-cli/src/hub/server.rs `include_bytes!("../../../hub-web/dist/…")`).
// A file the build emits but the hub does not embed is never served: a lazy
// chunk would 404 in production while every dev server served it. This pins
// dist/ to exactly the embedded set, so the next stray chunk fails here.
import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const dist = new URL("../dist/", import.meta.url);
const server = new URL("../../cas-cli/src/hub/server.rs", import.meta.url);

describe("committed dist", () => {
  it("contains exactly the files the hub embeds", () => {
    const embedded = [...readFileSync(server, "utf8").matchAll(/include_bytes!\("\.\.\/\.\.\/\.\.\/hub-web\/dist\/([^"]+)"\)/g)].map((match) => match[1]);
    expect(embedded.length).toBeGreaterThan(0);
    const built = readdirSync(dist).filter((name) => !name.startsWith("."));
    expect(built.toSorted()).toEqual([...new Set(embedded)].toSorted());
  });

  it("carries no dynamic import the hub could not serve", () => {
    const app = readFileSync(new URL("app.js", dist), "utf8");
    expect(app).not.toMatch(/import\(\s*["'`]\.\/?chunk-/);
    expect(app).not.toMatch(/import\(\s*["']crypto["']\s*\)/);
  });
});
