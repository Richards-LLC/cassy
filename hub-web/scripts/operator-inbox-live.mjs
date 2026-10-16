#!/usr/bin/env node
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { readdir, readFile, mkdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { liveOptions } from "./operator-inbox-live-safety.mjs";
const options = liveOptions(process.env); // Refuse before Playwright, credentials, files or HTTP.
const child = spawn(process.execPath, [fileURLToPath(new URL("../node_modules/@playwright/test/cli.js", import.meta.url)), "test", "--config", "playwright.operator-inbox-live.config.ts"], {
  cwd: fileURLToPath(new URL("../", import.meta.url)), env: process.env, stdio: "inherit",
});
for (const signal of ["SIGTERM", "SIGINT"]) process.once(signal, () => child.kill(signal));
child.once("error", () => { console.error("live inbox Playwright launch failed"); process.exitCode = 1; });
child.once("exit", async (code) => {
  process.exitCode = code ?? 1;
  try {
    const { scrubQaTraceFile } = await import("../../scripts/visual-qa.mjs");
    const results = join(options.root, "private-playwright-results");
    let secrets = [];
    try { secrets = JSON.parse(await readFile(join(options.root, "trace-secrets.json"), "utf8")); } catch {}
    const qa = join(options.artifacts, "qa"); await mkdir(qa, { recursive: true });
    for (const entry of await readdir(results, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      try { await scrubQaTraceFile(join(results, entry.name, "trace.zip"), join(qa, "trace.zip"), { secrets }); }
      catch (error) { if (error.code !== "ENOENT" && !String(error).includes("ENOENT")) throw error; }
    }
    // Native cleanup removes its credentials/children first. Retain root on a
    // cleanup failure for an operator retry; never remove another run's root.
    const cleanup = JSON.parse(await readFile(join(options.artifacts, "cleanup.json"), "utf8"));
    if (cleanup.failures.length === 0 && cleanup.remaining_active.length === 0) await rm(options.root, { recursive: true, force: true });
  } catch { console.error("live trace/cleanup packaging failed; private files retained outside artifacts"); process.exitCode = 1; }
});
