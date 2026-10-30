import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { HubDouble } from "./hub-double";
import { journeyNow, journeyStamp, JOURNEY_TIMEZONE } from "./clock";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import { once } from "node:events";

const PNG = readFileSync(new URL("../media/evidence.png", import.meta.url)).toString("base64");
for (const [width, height, colorScheme] of [[1440, 800, "light"], [1440, 800, "dark"], [390, 844, "light"], [390, 844, "dark"]] as const) {
  test.describe(`composer paste ${width} ${colorScheme}`, () => {
    test.use({ viewport: { width, height }, colorScheme });
    const run: Parameters<typeof test>[2] = async ({ page, journey, browser, baseURL }) => {
      test.setTimeout(120_000);
      const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
      const uploads = new Map<string, { name: string; data: string; path: string }>();
      let interrupted = false;
      const attempts: string[] = [];
      await page.route("https://*.test/v1/sessions/*/uploads/*", async route => {
        const id = route.request().url().split("/").at(-1)!;
        const body = route.request().postDataJSON() as { name: string; data: string };
        if (body.name === "unsupported.bin" || body.name === "orphan.png") {
          const missing = body.name === "orphan.png";
          await route.fulfill({ status: missing ? 403 : 415,
            json: missing ? { error: "session_denied", detail: "This session is not available on this machine." }
              : { error: "unsupported_file", detail: "This file type is not supported." },
            headers: { "access-control-allow-origin": "*" } });
          return;
        }
        attempts.push(id);
        const prior = uploads.get(id);
        if (prior) expect(body).toEqual({ name: prior.name, data: prior.data });
        const stored = prior ?? { ...body, path: `/fixture/project/artifacts/art-${id}/${body.name}` };
        uploads.set(id, stored);
        hub.storeComposerUpload("atlas", PELICAN, `art-${id}`, body.name.endsWith("png") ? "image/png" : "text/plain", Buffer.from(body.data, "base64"));
        if (!interrupted) { interrupted = true; await route.abort("connectionreset"); return; }
        await route.fulfill({ json: { artifact_id: `art-${id}`, path: stored.path, mime: body.name.endsWith("png") ? "image/png" : "text/plain" }, headers: { "access-control-allow-origin": "*" } });
      });
      let sentText = "";
      let queued = 0;
      const field = page.getByRole("textbox", { name: "Your message" });
      const send = page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true });
      await journey.stage("Paste plain text inline", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
        await field.focus();
        await page.evaluate(() => navigator.clipboard.writeText("A pasted message\nwith a second line"));
        await field.press("ControlOrMeta+V");
        await expect(field).toHaveValue("A pasted message\nwith a second line");
      });
      await journey.stage("Preview a long paste, screenshot and picked file together", async () => {
        await field.evaluate((element, png) => {
          const text = new DataTransfer(); text.setData("text/plain", "Long pasted content ".repeat(1000));
          element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: text, bubbles: true, cancelable: true }));
          const image = new DataTransfer();
          image.items.add(new File([Uint8Array.from(atob(png), c => c.charCodeAt(0))], "screenshot.png", { type: "image/png" }));
          element.dispatchEvent(new ClipboardEvent("paste", { clipboardData: image, bubbles: true, cancelable: true }));
        }, PNG);
        const chooser = page.waitForEvent("filechooser");
        await page.getByRole("button", { name: "Attach files", exact: true }).click();
        await (await chooser).setFiles({ name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("Project file from the phone picker") });
        await expect(page.getByRole("button", { name: "Remove pasted-text.txt", exact: true })).toBeVisible();
        await expect(page.getByRole("button", { name: "Remove screenshot.png", exact: true })).toBeVisible();
        await expect(page.locator(".composer-file img")).toBeVisible();
        await expect(page.getByRole("button", { name: "Remove notes.txt", exact: true })).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      });
      await journey.stage("A connection loss keeps the files visibly, then reuses the upload identity", async () => {
        await send.click();
        await expect(page.locator(".composer-uploads")).toContainText("Files were not sent");
        await expect(page.locator("#message-status")).toBeHidden();
        await expect(page.getByRole("button", { name: "Remove screenshot.png", exact: true })).toBeVisible();
        expect(uploads.size).toBe(1);
        const next = hub.nextSend();
        await send.click();
        const message = await next;
        expect(message.text).toContain("A pasted message\nwith a second line");
        expect(uploads.size).toBe(3);
        for (const upload of uploads.values()) expect(message.text).toContain(upload.path);
        expect(attempts[0]).toBe(attempts[1]);
        sentText = message.text;
        queued = hub.deliverLatest(PELICAN);
        await expect(page.locator(".composer-uploads")).toBeHidden();
        await expect(page.locator("#message-status")).toBeHidden();
        await expect(field).toHaveValue("");
        if (colorScheme === "light") {
          await page.screenshot({ path: `${process.env.JOURNEY_RECEIPTS ?? "e2e/.results"}/sent-attachments-${width}.png` });
        }
      });
      await journey.stage("Sent files show their names and an image preview after reload and on another phone", async () => {
        // cas-195a: a permissive ticket/content double hid the real backend bug.
        const imageId = `art-${[...uploads.entries()].find(([, upload]) => upload.name === "screenshot.png")![0]}`;
        const denied = await page.evaluate(async ({ imageId, session }) => {
          const paths = [
            `/v1/sessions/${session}/artifacts/art-unknown/ticket`,
            `/v1/sessions/foreign-session/artifacts/${imageId}/ticket`,
            `/v1/sessions/${session}/artifacts/${imageId}/content?t=unknown`,
          ];
          return Promise.all(paths.map(async path => (await fetch(`https://atlas.test${path}`, { method: path.includes("/ticket") ? "POST" : "GET" })).status));
        }, { imageId, session: PELICAN });
        expect(denied).toEqual([404, 404, 401]);
        const assertFiles = async (target: typeof page) => {
          const log = target.getByRole("log");
          await expect(log).not.toContainText("/fixture/project/");
          await expect(log.locator('a.sheet').filter({ hasText: "notes.txt" })).toBeVisible();
          const image = log.getByRole("img", { name: "screenshot.png", exact: true });
          await image.scrollIntoViewIfNeeded();
          await expect(image).toBeVisible();
          await expect.poll(() => image.evaluate((node: HTMLImageElement) => node.naturalWidth)).toBe(640);
          await expect(log.locator('a.sheet').filter({ hasText: "screenshot.png" }).locator('.plate img')).toBeVisible();
          expect(await image.evaluate((node) => node.getBoundingClientRect().width)).toBe(48);
          expect(await target.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        };
        await assertFiles(page);
        await page.screenshot({ path: `${process.env.JOURNEY_RECEIPTS ?? "e2e/.results"}/sent-attachments-${width}-${colorScheme}.png` });
        const popup = page.waitForEvent("popup");
        await page.getByRole("log").locator('a.sheet').filter({ hasText: "screenshot.png" }).click();
        const preview = await popup;
        await preview.waitForURL(/\/artifacts\/art-.*\/content\?t=/);
        await preview.close();
        await page.reload();
        await assertFiles(page);
        // A fresh pairing has none of the sender's object URLs or upload state.
        if (width === 1440 && colorScheme === "light") {
          const phone = await browser.newContext({ viewport: { width: 390, height: 844 }, colorScheme, timezoneId: JOURNEY_TIMEZONE });
          try {
            const peer = await phone.newPage();
            await peer.clock.install({ time: journeyNow() });
            const remote = new HubDouble(peer, { machines: [ATLAS], paired: ["atlas"], composerUploads: hub.composerUploads, history: { [PELICAN]: [{
              messages: [{ notification_id: queued, target: "supervisor", text: sentText, state: "delivered", stamped: true, device_id: "sender-device", session: PELICAN, at: journeyStamp() }],
              replies: [], has_earlier: false,
            }] } });
            await remote.install();
            await peer.goto(baseURL!); await remote.seedPaired(); await peer.reload();
            await peer.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(peer.locator(".conversation-row")).click();
            await assertFiles(peer);
            await peer.screenshot({ path: `${process.env.JOURNEY_RECEIPTS ?? "e2e/.results"}/sent-attachments-second-phone.png` });
          } finally { await phone.close(); }
        }
      });
      await journey.stage("A missing preview stays on its own file card", async () => {
        // Forward a real HTTP 404 from this run's static server. The ticket and
        // artifact request still take the normal application path.
        const missing = await page.request.get(`${new URL(baseURL!).origin}/commander/attachment-preview-missing.png`);
        expect(missing.status()).toBe(404);
        await page.route(`https://atlas.test/v1/sessions/${PELICAN}/artifacts/*/content?*`, route => route.fulfill({ response: missing }));
        await page.reload();
        const sent = page.getByRole("log").locator('a.sheet').filter({ hasText: "screenshot.png" });
        await expect(sent.locator('.preview-note')).toHaveText("Preview unavailable");
        await expect(sent.locator('.rich-media')).toBeHidden();
        hub.supervisorSays(PELICAN, "This screenshot is unavailable too.", { attachments: [{ artifact_id: "art-failed-preview", name: "Unavailable.png", mime: "image/png", size_bytes: 2048, sha256: "9f".repeat(32) }] });
        const received = page.getByRole("log").locator('a.sheet').filter({ hasText: "Unavailable.png" });
        await expect(received.locator('.preview-note')).toHaveText("Preview unavailable");
        await expect(received.locator('.rich-media')).toBeHidden();
        for (const card of [sent, received]) {
          await card.scrollIntoViewIfNeeded();
          expect((await card.boundingBox())!.height).toBeLessThan(104);
        }
        await page.screenshot({ path: `${process.env.JOURNEY_RECEIPTS ?? "e2e/.results"}/unavailable-${width}-${colorScheme}.png` });
      });
      await journey.stage("A slow 10 MiB photo shows byte progress, then Saving, then Delivered", async () => {
        // A real loopback HTTP upload supplies browser byte events. Only storage
        // and message delivery use the hub double; no progress events are injected.
        let releaseSaving!: () => void;
        const saving = new Promise<void>(resolve => { releaseSaving = resolve; });
        let receivedBytes = 0;
        const server = createServer(async (request, response) => {
          response.setHeader("access-control-allow-origin", "*");
          response.setHeader("access-control-allow-headers", "*");
          if (request.method === "OPTIONS") { response.end(); return; }
          const chunks: Buffer[] = [];
          for await (const chunk of request) chunks.push(Buffer.from(chunk));
          const body = JSON.parse(Buffer.concat(chunks).toString()) as { name: string; data: string };
          const bytes = Buffer.from(body.data, "base64"); receivedBytes = bytes.length;
          hub.storeComposerUpload("atlas", PELICAN, "art-slow-photo", "image/png", bytes);
          await saving;
          response.setHeader("content-type", "application/json");
          response.end(JSON.stringify({ artifact_id: "art-slow-photo", path: "/fixture/project/artifacts/art-slow-photo/slow-photo.png", mime: "image/png" }));
        });
        server.listen(0, "127.0.0.1"); await once(server, "listening");
        const port = (server.address() as { port: number }).port;
        const pattern = "https://*.test/v1/sessions/*/uploads/*";
        const forward = async (route: import("@playwright/test").Route) => {
          if (route.request().postDataJSON()?.name !== "slow-photo.png") return route.fallback();
          return route.continue({ url: `http://127.0.0.1:${port}/upload` });
        };
        const cdp = await page.context().newCDPSession(page);
        try {
          await page.route(pattern, forward);
          await cdp.send("Network.enable");
          // 1 MiB/s keeps the browser walk bounded; the 512 kbit/s + 25%
          // deadline boundary is covered by the deterministic transport test.
          await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: 1024 * 1024 });
          const bytes = Buffer.alloc(10 * 1024 * 1024); Buffer.from(PNG, "base64").copy(bytes);
          const chooser = page.waitForEvent("filechooser");
          await page.getByRole("button", { name: "Attach files", exact: true }).click();
          await (await chooser).setFiles({ name: "slow-photo.png", mimeType: "image/png", buffer: bytes });
          const next = hub.nextSend();
          await send.click();
          const progress = page.getByRole("progressbar", { name: "Upload slow-photo.png" });
          await expect(progress).toBeVisible();
          await expect.poll(async () => Number(await progress.getAttribute("aria-valuenow"))).toBeGreaterThan(0);
          expect(Number(await progress.getAttribute("aria-valuenow"))).toBeLessThan(100);
          await page.screenshot({ path: `${process.env.JOURNEY_RECEIPTS ?? "e2e/.results"}/upload-progress-${width}-${colorScheme}.png` });
          await expect(page.locator(".composer-uploads")).toContainText("Saving slow-photo.png", { timeout: 30_000 });
          expect(receivedBytes).toBe(10 * 1024 * 1024);
          await page.screenshot({ path: `${process.env.JOURNEY_RECEIPTS ?? "e2e/.results"}/upload-saving-${width}-${colorScheme}.png` });
          releaseSaving();
          expect((await next).text).toContain("art-slow-photo/slow-photo.png");
          hub.deliverLatest(PELICAN);
          await expect(page.locator(".composer-uploads")).toBeHidden();
          await expect(page.getByRole("log").locator(".conversation-turn").filter({ hasText: "slow-photo.png" }).locator(".conversation-delivered")).toHaveText("Delivered");
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
        } finally {
          releaseSaving();
          await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1 });
          await cdp.detach(); await page.unroute(pattern, forward);
          server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve()));
        }
      });
      await journey.stage("A refusal names its file and keeps the hub's reason", async () => {
        const pick = async (files: { name: string; mimeType: string; buffer: Buffer }[]) => {
          const chooser = page.waitForEvent("filechooser");
          await page.getByRole("button", { name: "Attach files", exact: true }).click();
          await (await chooser).setFiles(files);
        };
        await pick([{ name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("notes") },
          { name: "unsupported.bin", mimeType: "application/octet-stream", buffer: Buffer.from([0, 1, 2]) }]);
        await send.click();
        await expect(page.locator(".upload-error")).toContainText("unsupported.bin: This file type is not supported.");
        await expect(page.getByText(/Files were not sent\. unsupported.bin:/)).toHaveCount(1);
        await expect(page.locator("#message-status")).toBeHidden();
        await expect(page.locator(".upload-error")).not.toContainText("reconnect");
        await page.getByRole("button", { name: "Remove unsupported.bin", exact: true }).click();
        await pick([{ name: "orphan.png", mimeType: "image/png", buffer: Buffer.from(PNG, "base64") }]);
        await send.click();
        await expect(page.locator(".upload-error")).toContainText("orphan.png: This session is not available on this machine.");
        await expect(page.getByText(/Files were not sent\. orphan.png:/)).toHaveCount(1);
        await expect(page.locator("#message-status")).toBeHidden();
        await expect(page.locator(".upload-error")).toContainText("Your files are kept here.");
        await expect(page.getByRole("button", { name: "Remove orphan.png", exact: true })).toBeVisible();
      });
    };
    if (width === 1440 && colorScheme === "light") test("HUB-J24 paste content into the composer", run);
    else test(`HUB-J24 paste content ${width} ${colorScheme}`, journeyPart, run);
  });
}
