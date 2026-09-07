import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const app = fileURLToPath(new URL("..", import.meta.url));

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
