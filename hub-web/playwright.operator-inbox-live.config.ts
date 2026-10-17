import { defineConfig } from "@playwright/test";
import { liveOptions } from "./scripts/operator-inbox-live-safety.mjs";
const options = liveOptions(process.env);
export default defineConfig({
  testDir: "./scripts", testMatch: "operator-inbox-live.spec.mjs", workers: 1, retries: 0,
  timeout: 240_000, expect: { timeout: 30_000 }, reporter: "list",
  outputDir: `${options.root}/private-playwright-results`,
  use: { channel: "chromium", trace: { mode: "on", snapshots: { dom: true, aria: true, screen: true }, screenshots: false, sources: true } },
  // Native control is started through the factory server registry first.
  // Raw traces contain device grants: the test's afterEach scrubs them.
});
