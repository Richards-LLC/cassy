import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { checkoutPorts } from "./e2e/checkout-ports";

// One Playwright config for hub-web, two projects:
// - fixtures (cas-d7b7 Test Agents spike): specs under e2e/ against the Vite
//   fixture site, real src/ renderers with fixture data and no hub daemon.
// - journeys (cas-9be7): one test per user journey in docs/qa/journeys.md,
//   driving the committed production bundle (dist/) at /commander/ through a
//   hub protocol double. Receipts: e2e/.results/journeys/<ID>/ or
//   $JOURNEY_RECEIPTS (scripts/journey-eval.sh).
//
// Ports (cas-00ad): HUB_E2E_PORT / HUB_JOURNEY_PORT when set (use a pair in
// 20000–32767 per concurrent run), else this checkout's own pair from its
// path. Servers are never reused, so a run always serves this checkout's
// fixtures and dist; a port someone else holds fails the run loudly.
const defaults = checkoutPorts(fileURLToPath(new URL(".", import.meta.url)));
const port = Number(process.env.HUB_E2E_PORT ?? defaults.fixtures);
const origin = `http://127.0.0.1:${port}`;
const journeyPort = Number(process.env.HUB_JOURNEY_PORT ?? defaults.journeys);
const journeyOrigin = `http://127.0.0.1:${journeyPort}`;

export default defineConfig({
  testDir: "./e2e",
  testMatch: "**/*.spec.ts",
  outputDir: process.env.JOURNEY_OUTPUT ?? "./e2e/.results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"], ["json", { outputFile: `${process.env.JOURNEY_OUTPUT ?? "e2e/.results"}/report.json` }]],
  use: { baseURL: origin, trace: "retain-on-failure" },
  projects: [
    {
      name: "chromium",
      testIgnore: "journeys/**",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "journeys",
      testDir: "./e2e/journeys",
      testMatch: "**/*.journey.ts",
      fullyParallel: false,
      timeout: 60_000,
      use: {
        ...devices["Desktop Chrome"],
        baseURL: `${journeyOrigin}/commander/`,
        colorScheme: "light",
        // chromium-headless-shell crashes the renderer when a conversation mounts
        // its terminal surface; the full Chromium build does not.
        channel: "chromium",
        permissions: ["clipboard-read", "clipboard-write"],
        // screenshots:false keeps the screencast receipt full size (the trace
        // filmstrip shares it and caps frames at 800 px); screen snapshots stay.
        trace: { mode: "on", snapshots: { dom: true, aria: true, screen: true }, sources: true, screenshots: false },
        video: "off",
      },
    },
  ],
  webServer: [
    {
      command: `npx vite fixtures --base / --port ${port} --strictPort --host 127.0.0.1`,
      url: `${origin}/`,
      reuseExistingServer: false,
    },
    {
      command: `node e2e/journeys/serve-dist.mjs ${journeyPort}`,
      url: `${journeyOrigin}/commander/`,
      reuseExistingServer: false,
    },
  ],
});
