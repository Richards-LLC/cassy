import { defineConfig } from "@playwright/test";

const port = Number(process.env.REAL_HUB_CONTROL_PORT ?? 29925);
export default defineConfig({
  testDir: "./e2e/journeys",
  testMatch: "real-hub.journey.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  outputDir: process.env.JOURNEY_OUTPUT ?? "e2e/.results/real-hub",
  reporter: "list",
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    channel: "chromium",
    ignoreHTTPSErrors: true,
    viewport: { width: 1280, height: 800 },
    permissions: ["local-network-access"],
    trace: "off", // The journey writes a scrubbed trace, including real credentials.
    launchOptions: {
      args: [
        "--ignore-certificate-errors",
        "--host-resolver-rules=MAP journey-hub.ts.net 127.0.0.1",
        `--ip-address-space-overrides=127.0.0.1:${port}=public`,
      ],
    },
  },
  // Start real-hub-server.mjs through the factory server registry first. Never
  // reuse an arbitrary production hub or let Playwright own an untracked server.
});
