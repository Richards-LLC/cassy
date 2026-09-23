import { defineConfig, devices } from "@playwright/test";

// One Playwright config for hub-web, two projects:
// - fixtures (cas-d7b7 Test Agents spike): specs under e2e/ against the Vite
//   fixture site, real src/ renderers with fixture data and no hub daemon.
// - journeys (cas-9be7): one test per user journey in docs/qa/journeys.md,
//   driving the committed production bundle (dist/) at /commander/ through a
//   hub protocol double. Receipts: e2e/.results/journeys/<ID>/ or
//   $JOURNEY_RECEIPTS (scripts/journey-eval.sh).
const port = Number(process.env.HUB_E2E_PORT ?? 4791);
const origin = `http://127.0.0.1:${port}`;
const journeyPort = Number(process.env.HUB_JOURNEY_PORT ?? 4792);
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
        trace: { mode: "on", snapshots: { dom: true, aria: true, screen: true } },
        video: "off",
      },
    },
  ],
  webServer: [
    {
      command: `npx vite fixtures --base / --port ${port} --strictPort --host 127.0.0.1`,
      url: `${origin}/`,
      reuseExistingServer: !process.env.CI,
    },
    {
      command: `node e2e/journeys/serve-dist.mjs ${journeyPort}`,
      url: `${journeyOrigin}/commander/`,
      reuseExistingServer: !process.env.CI,
    },
  ],
});
