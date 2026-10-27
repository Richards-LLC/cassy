// The same named journey, with Commander assets served by a registered real hub.
import { defineConfig } from "@playwright/test";
import base from "./playwright.config";

const value = process.env.HUB_RICH_HUB_URL;
if (!value) throw new Error("Set HUB_RICH_HUB_URL to the registered candidate's /commander/ URL");
const url = new URL(value);
if (!["localhost", "127.0.0.1", "[::1]"].includes(url.hostname) || !["http:", "https:"].includes(url.protocol) || url.pathname !== "/commander/" || url.username || url.password || url.search || url.hash) throw new Error("HUB_RICH_HUB_URL must name a local candidate at /commander/");

export default defineConfig(base, {
  projects: [{
    ...base.projects!.find((project) => project.name === "journeys")!,
    testMatch: "rich-content.journey.ts",
    use: { ...base.projects!.find((project) => project.name === "journeys")!.use, baseURL: url.href },
  }],
  webServer: [], // The supervisor owns the registered candidate and its lifetime.
});
