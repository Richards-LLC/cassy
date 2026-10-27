import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./journeys",
  testMatch: "conversation-open.perf.ts",
  workers: 1,
  retries: 0,
  timeout: 180_000,
  outputDir: `${process.env.CONVERSATION_PERF_OUTPUT}/runner`,
  reporter: "list",
  use: { channel: "chromium", trace: "off", timezoneId: "America/New_York" },
});
