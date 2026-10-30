import { defineConfig, devices } from "@playwright/test";
import { fileURLToPath } from "node:url";
import { checkoutPorts } from "./e2e/checkout-ports.mjs";
import { JOURNEY_TIMEZONE } from "./e2e/journeys/clock";

// One Playwright config for hub-web, two projects:
// - fixtures (cas-d7b7 Test Agents spike): specs under e2e/ against the Vite
//   fixture site, real src/ renderers with fixture data and no hub daemon.
// - journeys (cas-9be7): one test per user journey in docs/qa/journeys.md,
//   driving a source-bound scratch production build at /commander/ through a
//   hub protocol double. Receipts: e2e/.results/journeys/<ID>/ or
//   $JOURNEY_RECEIPTS (scripts/journey-eval.sh).
//
// Ports (cas-00ad): HUB_E2E_PORT / HUB_JOURNEY_PORT when set (use a pair in
// 20000–32767 per concurrent run), else this checkout's own pair from its
// path. Servers are never reused, so a run always serves this checkout's
// fixtures and scratch build; a port someone else holds fails the run loudly.
//
// Readiness is the launched server's own "listening" line, never a probe of the
// URL: a URL probe cannot tell this run's server from a concurrent caller's on
// the same port, so a run whose own server lost the bind could attach to the
// winner's before its server's exit was seen (cas-9e97).
const defaults = checkoutPorts(fileURLToPath(new URL(".", import.meta.url)));
const port = Number(process.env.HUB_E2E_PORT ?? defaults.fixtures);
const origin = `http://127.0.0.1:${port}`;
const journeyPort = Number(process.env.HUB_JOURNEY_PORT ?? defaults.journeys);
const journeyOrigin = `http://127.0.0.1:${journeyPort}`;
const listeningOn = (url: string) => new RegExp(url.replaceAll(".", "\\."));

export default defineConfig({
  testDir: "./e2e",
  testMatch: "**/*.spec.ts",
  outputDir: process.env.JOURNEY_OUTPUT ?? "./e2e/.results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  // cas-4ce5/cas-84a2: the merge queue runs on the factory host's self-hosted
  // runners beside the fleet; Playwright's default (half the cores: 16 here)
  // put 16 recording Chromiums on a loaded box and journeys stalled. Six keeps
  // the suite to a few minutes without starving the renderers.
  // cas-61dc: locally, host memory admission serializes suites, so each suite
  // keeps one browser worker unless the caller (journey-eval.sh) asks for more.
  workers: process.env.CI ? 6 : 1,
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
      testIgnore: "real-hub.journey.ts", // Requires the separately registered disposable real hub.
      fullyParallel: false,
      timeout: 60_000,
      use: {
        ...devices["Desktop Chrome"],
        baseURL: `${journeyOrigin}/commander/`,
        colorScheme: "light",
        timezoneId: JOURNEY_TIMEZONE,
        // chromium-headless-shell crashes the renderer when a conversation mounts
        // its terminal surface; the full Chromium build does not.
        channel: "chromium",
        permissions: ["clipboard-read", "clipboard-write"],
        // The screencast and stage captures already carry the visual frames;
        // omit trace screenshots and screen snapshots to avoid storing them twice.
        trace: { mode: "on", snapshots: { dom: true, aria: true, screen: false }, sources: true, screenshots: false },
        video: "off",
      },
    },
  ],
  webServer: [
    {
      command: `npx vite fixtures --base / --port ${port} --strictPort --host 127.0.0.1`,
      wait: { stdout: listeningOn(`Local:\\s+${origin}/`) },
      reuseExistingServer: false,
    },
    {
      command: `node e2e/journeys/serve-dist.mjs ${journeyPort}`,
      wait: { stdout: listeningOn(`at ${journeyOrigin}/commander/`) },
      reuseExistingServer: false,
    },
  ],
});
