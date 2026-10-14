// cas-eaa3: public/cassy-tokens.css is the one token source Commander and
// Explorer share. It must be current with tokens.css and glass.css, hold only
// token blocks, prove its own integrity, and the sync script must vendor it
// and flag a hand edit or drift.
import { createHash } from "node:crypto";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const root = fileURLToPath(new URL("..", import.meta.url));
const artifact = join(root, "public/cassy-tokens.css");
const sync = join(root, "scripts/sync-cassy-tokens.mjs");
const run = (args: string[]) => spawnSync(process.execPath, [sync, ...args], { encoding: "utf8" });

function split(css: string) {
  const end = css.indexOf("*/\n");
  return { header: css.slice(0, end + 3), body: css.slice(end + 3) };
}

describe("cassy-tokens.css", () => {
  it("is current with tokens.css and glass.css", () => {
    expect(() => execFileSync(process.execPath, [join(root, "scripts/build-cassy-tokens.mjs"), "--check"], { encoding: "utf8" })).not.toThrow();
  });

  it("holds only scheme token blocks, the house and Glass roles, under an intact hash", () => {
    const css = readFileSync(artifact, "utf8");
    const { header, body } = split(css);
    expect(header).toContain(`cassy-tokens-sha256: ${createHash("sha256").update(body).digest("hex")}`);
    for (const role of ["--color-action", "--bg-root", "--ink", "--look-send", "--look-glass", "--look-aurora", "--font-ui", "--space-4"]) expect(body).toContain(`${role}:`);
    // No component rules: every rule selects the document, never a class or element.
    const selectors = [...body.replace(/\/\*[\s\S]*?\*\//g, "").matchAll(/^\s*([^@{}\s][^{}]*)\{/gm)].map((m) => m[1].trim());
    expect(selectors.length).toBeGreaterThan(5);
    for (const selector of selectors) expect(selector).toMatch(/^(?:(?:html|:root)(?:\[[^\]]+\]|:root|:not\([^)]*\))*(?:,\s*)?)+$/);
    for (const line of body.split("\n").filter((l) => /^\s+[a-z-]+\s*:/.test(l))) expect(line.trim()).toMatch(/^(--[\w-]+|color-scheme)\s*:/);
  });

  it("syncs a copy and flags a hand edit or drift", () => {
    const dir = mkdtempSync(join(tmpdir(), "cassy-tokens-"));
    const vendored = join(dir, "cassy-tokens.css");
    expect(run(["--from", artifact, "--to", vendored]).status).toBe(0);
    expect(readFileSync(vendored, "utf8")).toBe(readFileSync(artifact, "utf8"));
    const current = run(["--from", artifact, "--to", vendored, "--check"]);
    expect([current.status, current.stdout]).toEqual([0, expect.stringContaining("is current")]);

    writeFileSync(vendored, readFileSync(artifact, "utf8").replace("--space-4: 16px", "--space-4: 15px"));
    const edited = run(["--from", artifact, "--to", vendored, "--check"]);
    expect([edited.status, edited.stdout]).toEqual([1, expect.stringContaining("edited by hand")]);

    // A newer upstream with its own intact header is drift for the old copy.
    const { body } = split(readFileSync(artifact, "utf8"));
    const newerBody = body.replace("--space-4: 16px", "--space-4: 18px");
    const newer = join(dir, "upstream.css");
    writeFileSync(newer, `/* cassy-tokens-sha256: ${createHash("sha256").update(newerBody).digest("hex")}\n */\n${newerBody}`);
    expect(run(["--from", artifact, "--to", vendored]).status).toBe(0);
    const drift = run(["--from", newer, "--to", vendored, "--check"]);
    expect([drift.status, drift.stdout]).toEqual([1, expect.stringContaining("run without --check to update")]);

    // A source whose header does not match its body is refused outright.
    writeFileSync(newer, `/* cassy-tokens-sha256: ${"0".repeat(64)}\n */\n${newerBody}`);
    expect(run(["--from", newer, "--to", vendored]).status).toBe(2);
    expect(run(["--from", "http://example.invalid/cassy-tokens.css", "--to", vendored]).status).toBe(2);
  });

  it("prints usage for --help and for bad arguments instead of a stack trace (QA)", () => {
    const help = run(["--help"]);
    expect([help.status, help.stdout]).toEqual([0, expect.stringContaining("Usage: node sync-cassy-tokens.mjs --to <path>")]);
    for (const args of [["--bogus"], []]) {
      const bad = run(args);
      expect(bad.status).toBe(2);
      expect(bad.stderr).toContain("Usage:");
      expect(bad.stderr).not.toMatch(/\n\s+at /);
    }
  });
});
