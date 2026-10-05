import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { test, expect, journeyPart } from "./journey";
import { ATLAS } from "./world";
import { HubDouble } from "./hub-double";
import { ProtocolClock } from "./protocol-clock";

test("HUB-J12 named connection cause and safe export recover together (cas-2b3a5)", journeyPart, async ({ page, journey }) => {
  const clock = new ProtocolClock(page);
  const hub = new HubDouble(page, { machines: [ATLAS], paired: ["atlas"], multiplex: true, time: clock });
  await hub.install();
  await page.addInitScript(() => { Math.random = () => 0.5; });
  let blocked = false;
  await page.route("https://atlas.test/v1/**", route => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/v1/diagnostics") return route.fulfill({ json: {
      tailscale_status: { Self: { HostName: "SECRET-HOST" } }, prompt: "SECRET-PROMPT", credential: "SECRET-TOKEN",
      connection_recovery: { counts: [{ category: "events", preflight: false, status: 200, count: 3, request_id: "a1111111-1111-4111-8111-111111111111" }], refusals: { revoked: 2, "SECRET-TOKEN": 9 } },
    } });
    if (blocked && path !== "/v1/health") return route.abort("failed");
    return route.fallback();
  });
  await page.goto("./"); await hub.seedPaired(); await clock.start(); await page.goto("./");
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
  await expect(page.locator("#conversation-connection")).toHaveText(" · Live");
  await journey.stage("A healthy public route does not hide an opaque authenticated failure", async () => {
    blocked = true;
    await hub.down("atlas", { sockets: "close" }); await hub.up("atlas");
    await clock.advance(1_000);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Reconnecting");
    await page.getByRole("button", { name: "Connection details", exact: true }).click();
    await expect(page.getByRole("dialog", { name: "Connection log" })).toBeVisible();
    await expect(page.locator(".connection-log-summary")).toContainText("Network or browser policy blocked the request (browser)");
    await expect(page.locator(".connection-log-summary")).toContainText("Next retry in");
    await expect(page.locator(".connection-log-summary")).toContainText("Last successful connection:");
    await expect(page.getByRole("button", { name: "Export safe diagnostics" })).toBeEnabled();
    const close = page.getByRole("button", { name: "Close connection log" });
    await expect(close).toBeVisible();
    expect((await close.boundingBox())!.width).toBeGreaterThanOrEqual(44);
    await expect(page.locator("#connection-log")).toMatchAriaSnapshot(`- dialog "Connection log":
  - paragraph: Evidence ledger
  - heading "Connection log" [level=2]
  - button "Close connection log": ×
  - paragraph: /Network or browser policy blocked the request.*/
  - button "Export safe diagnostics"
  - text: /.*schema_version.*/`);
    const downloadPromise = page.waitForEvent("download");
    await page.getByRole("button", { name: "Export safe diagnostics" }).click();
    const download = await downloadPromise;
    const json = await readFile((await download.path())!, "utf8");
    expect(json).not.toMatch(/SECRET-|tailscale_status|Authorization|poll_secret|privateKey|prompt/);
    expect(Buffer.byteLength(json)).toBeLessThan(65_536);
    const report = JSON.parse(json);
    expect(report.transitions.some((row: { cause?: { code: string } }) => row.cause?.code === "network_or_browser_policy_unknown")).toBe(true);
    expect(report.hub.counts[0].request_id).toBe("a1111111-1111-4111-8111-111111111111");
    // Static capture of the real built dialog and exact dist CSS; interaction
    // stays in the journey. Scope polish checks away from the app behind it.
    const css = await readFile(new URL("../../dist/app.css", import.meta.url), "utf8");
    const markup = await page.locator("#connection-log").evaluate(element => element.outerHTML);
    await writeFile(join(process.env.JOURNEY_RECEIPTS!, "connection-log-snapshot.html"), `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Built Commander connection log snapshot</title><style>${css}</style><body>${markup}<script>document.documentElement.dataset.scheme=matchMedia('(prefers-color-scheme:dark)').matches?'dark':'light';const dialog=document.querySelector('dialog');dialog.removeAttribute('open');dialog.showModal();</script></body></html>`);
  });
  for (const width of [1280, 390]) for (const scheme of ["light", "dark"] as const) {
    await journey.stage(`Named evidence at ${width}px ${scheme}`, async () => {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
      await page.emulateMedia({ colorScheme: scheme });
      await expect(page.getByRole("button", { name: "Export safe diagnostics" })).toBeInViewport();
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      expect(await page.locator("#connection-log").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
    });
  }
  await journey.stage("Keyboard close restores focus; successful recovery is recorded", async () => {
    await page.keyboard.press("Escape");
    await expect(page.getByRole("button", { name: "Connection details", exact: true })).toBeFocused();
    blocked = false; await clock.advance(10_000);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Live");
    // Force a second short outage to open the user-facing log, then recover
    // with it open. The summary updates without repeatedly announcing a clock.
    blocked = true; await hub.down("atlas", { sockets: "close" }); await hub.up("atlas"); await clock.advance(1_000);
    await page.getByRole("button", { name: "Connection details", exact: true }).click();
    await expect(page.locator("#connection-log pre")).toContainText('"recovered": "network_or_browser_policy_unknown"');
    blocked = false; await clock.advance(10_000);
    await expect(page.locator(".connection-log-summary")).toContainText("No active failure measured");
  });
  for (const setting of ["reducedMotion", "forcedColors", "contrast"] as const) {
    await journey.stage(`Accessible log with ${setting}`, async () => {
      await page.emulateMedia(setting === "reducedMotion" ? { reducedMotion: "reduce" } : setting === "forcedColors" ? { forcedColors: "active" } : { contrast: "more" });
      await expect(page.getByRole("button", { name: "Close connection log" })).toBeInViewport();
      await expect(page.getByRole("button", { name: "Export safe diagnostics" })).toBeEnabled();
    });
  }
});
