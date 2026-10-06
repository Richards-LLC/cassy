import { defineConfig } from "@playwright/test";
import base from "./playwright.config";

// Run one variant at a time so each owns its journey receipts. This fixture
// regression lane retains the original desktop tests and adds native phone goals.
const layout = process.env.JOURNEY_LAYOUT ?? "phone";
const scheme = process.env.JOURNEY_SCHEME ?? "light";
if (layout !== "phone" && layout !== "desktop") throw new Error("JOURNEY_LAYOUT must be phone or desktop");
if (scheme !== "light" && scheme !== "dark") throw new Error("JOURNEY_SCHEME must be light or dark");
const journey = base.projects!.find(project => project.name === "journeys")!;

export default defineConfig({
  ...base,
  // Only the production bundle is needed, not the Vite component fixture site.
  webServer: Array.isArray(base.webServer) ? base.webServer.slice(1) : base.webServer,
  grep: [
    /HUB-J1 first open and pair a machine with a code$/,
    /HUB-J2 pair a machine from a cas hub pair link$/,
    /HUB-J2 pair a link that grants factory:manage, and see a pairing without it named/,
    /HUB-J3 find the conversation that needs me$/,
    /HUB-J3 on a Mac, every surface names the palette chord/,
    /HUB-J5 reply by typing$/,
    /HUB-J7 answer a question in the thread$/,
    /HUB-J7 a machine clock ahead: the first visit and a reload agree, and the row ages/,
    /HUB-J8 switch between machines without losing my place$/,
    /HUB-J10 switch to dark and keep reading$/,
    /HUB-J11 the connection drops mid-conversation and recovers$/,
  ],
  projects: [{
    ...journey,
    name: `${layout}-${scheme}`,
    use: {
      ...journey.use,
      viewport: layout === "phone" ? { width: 390, height: 844 } : { width: 1280, height: 800 },
      hasTouch: layout === "phone",
      isMobile: layout === "phone",
      colorScheme: scheme,
    },
  }],
});
