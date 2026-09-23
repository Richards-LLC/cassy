import { defineConfig, devices } from "@playwright/test";

// Spike harness (cas-d7b7): Playwright Test Agents against the hub-web fixture
// site. Vite serves hub-web/fixtures in dev mode, so specs exercise real src/
// renderers with fixture data and no hub daemon.
const port = Number(process.env.HUB_E2E_PORT ?? 4791);
const origin = `http://127.0.0.1:${port}`;

export default defineConfig({
  testDir: "./e2e",
  testMatch: "**/*.spec.ts",
  outputDir: "./e2e/.results",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: [["list"], ["json", { outputFile: "e2e/.results/report.json" }]],
  use: { baseURL: origin, trace: "retain-on-failure" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `npx vite fixtures --base / --port ${port} --strictPort --host 127.0.0.1`,
    url: `${origin}/`,
    reuseExistingServer: !process.env.CI,
  },
});
