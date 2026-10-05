import { test, expect, journeyPart } from "./journey";
import { HubDouble } from "./hub-double";
import { ATLAS, PELICAN } from "./world";
import { journalRows } from "./commander-journal-storage";
import { journeyNow } from "./clock";
import type { Page } from "@playwright/test";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

async function choose(page: Page) {
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
}
async function send(page: Page, text: string) {
  await page.getByRole("textbox", { name: "Your message" }).fill(text);
  await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
}

async function captureReceiptSurface(page: Page, state: "stored" | "forwarded") {
  const qa = process.env.DELIVERY_QA;
  if (!qa) return;
  await mkdir(qa, { recursive: true });
  const viewport = page.viewportSize()!;
  for (const [size, width, height] of [["desktop", 1280, 800], ["phone", 390, 844]] as const) {
    for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height }); await page.emulateMedia({ colorScheme });
      await expect(page.getByRole("log")).toContainText(state === "stored" ? "Stored on this device" : "Forwarded · not stored on this device");
      await page.screenshot({ path: join(qa, `${state}-${colorScheme}-${size}.png`) });
    }
  }
  const html = await page.locator(".conversation-reading").evaluate(node => node.outerHTML);
  const css = await readFile("dist/app.css", "utf8");
  await writeFile(join(qa, `${state}.html`), `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${css}</style></head><body>${html}</body></html>`);
  for (const [name, query, media] of [["forced-colors", "(forced-colors: active)", { forcedColors: "active" }], ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }], ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }]] as const) {
    await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null, ...media });
    expect(await page.evaluate(query => matchMedia(query).matches, query)).toBe(true);
    await page.screenshot({ path: join(qa, `${state}-a11y-${name}.png`) });
  }
  await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null, colorScheme: "light" });
  await page.setViewportSize(viewport);
}

test("HUB-J12 atomic pending sends across two tabs and reload", journeyPart, async ({ page, context, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  const second = await context.newPage();
  await second.clock.install({ time: journeyNow() });
  const other = new HubDouble(second, { machines: [ATLAS], paired: ["atlas"], multiplex: true });
  const errors: string[] = [];
  second.on("pageerror", (error) => errors.push(error.message));
  await other.install();
  await journey.stage("Two tabs keep different messages during the same outage", async () => {
    await journey.open(); await choose(page);
    await second.goto(page.url()); await choose(second);
    await Promise.all([hub.down("atlas", { sockets: "close" }), other.down("atlas", { sockets: "close" })]);
    await expect(page.locator("#conversation-connection")).not.toHaveText(" · Live");
    await expect(second.locator("#conversation-connection")).not.toHaveText(" · Live");
    await Promise.all([send(page, "First tab keeps this"), send(second, "Second tab keeps that")]);
    await expect(page.getByRole("log").locator(".conversation-held")).not.toHaveCount(0);
    await expect(second.getByRole("log").locator(".conversation-held")).not.toHaveCount(0);
    await expect.poll(async () => (await journalRows(page, "sends")).filter((row) => row.send?.state === "held").map((row) => row.send!.text).sort()).toEqual(["First tab keeps this", "Second tab keeps that"]);
  });
  const releaseAttach = [hub.holdAttach(PELICAN), other.holdAttach(PELICAN)];
  await journey.stage("Both tabs reload the same two held client references", async () => {
    // The catalog is reachable again while both session attaches remain held.
    await Promise.all([hub.up("atlas"), other.up("atlas")]);
    await Promise.all([page.reload(), second.reload()]);
    await Promise.all([choose(page), choose(second)]);
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(2);
    await expect(second.getByRole("log").locator(".conversation-held")).toHaveCount(2);
  });
  await journey.stage("Both connections recover; each item crosses the wire once", async () => {
    for (const release of releaseAttach) release();
    await expect.poll(() => [...hub.sends, ...other.sends].map((row) => row.text).sort(), { timeout: 20_000 }).toEqual(["First tab keeps this", "Second tab keeps that"]);
    const sends = [...hub.sends, ...other.sends];
    expect(new Set(sends.map((row) => row.client_ref)).size).toBe(2);
    await Promise.all([page.reload(), second.reload()]);
    await choose(page); await choose(second);
    await expect(page.getByRole("log")).toContainText("2 messages not confirmed");
    await expect.poll(() => [...hub.sends, ...other.sends].length).toBe(2);
    expect(errors).toEqual([]);
  });
  await second.close();
});

test("HUB-J12 cancellation persists and cannot drain after reload", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  await journey.stage("Cancel an explicitly waiting message", async () => {
    await journey.open(); await choose(page);
    hub.upstreamLost(PELICAN);
    await send(page, "Cancel this waiting instruction");
    await page.getByRole("button", { name: "Cancel waiting message" }).click();
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(0);
    expect((await journalRows(page, "sends")).some((row) => row.send?.text === "Cancel this waiting instruction")).toBe(false);
  });
  await journey.stage("Reload and reconnect do not resurrect the cancelled client reference", async () => {
    await page.reload(); await choose(page);
    hub.upstreamBack(PELICAN);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Live", { timeout: 20_000 });
    await expect(page.getByText("Cancel this waiting instruction", { exact: true })).toHaveCount(0);
    expect(hub.sends.some((row) => row.text === "Cancel this waiting instruction")).toBe(false);
  });
});

test("HUB-J3 reply application ACK follows real IndexedDB commit and replay dedupes", journeyPart, async ({ page, journey }) => {
  await page.addInitScript(() => {
    const events: string[] = [];
    (window as unknown as { __receiptOrder: string[] }).__receiptOrder = events;
    const transact = IDBDatabase.prototype.transaction;
    IDBDatabase.prototype.transaction = function(stores, mode, options) {
      const tx = transact.call(this, stores, mode, options);
      if (mode === "readwrite" && (typeof stores === "string" ? [stores] : [...stores]).includes("replies")) tx.addEventListener("complete", () => events.push("reply-commit"));
      return tx;
    };
  });
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  // Routed sockets define send on the constructed instance. Observe the
  // transport actually used by the bundle, after routeWebSocket is installed.
  await page.addInitScript(() => {
    const Socket = window.WebSocket;
    window.WebSocket = class extends Socket {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        const write = this.send.bind(this);
        this.send = (data) => {
          if (typeof data === "string" && data.includes('"OperatorReplyPersisted"'))
            (window as unknown as { __receiptOrder: string[] }).__receiptOrder.push("application-ack");
          return write(data);
        };
      }
    };
  });
  let id = 0;
  await journey.stage("The reply is stored on this device before its application ACK", async () => {
    await journey.open(); await choose(page);
    await page.evaluate(() => { (window as unknown as { __receiptOrder: string[] }).__receiptOrder.length = 0; });
    id = hub.supervisorSays(PELICAN, "A durable reply on this device");
    await expect.poll(() => hub.persistedReplies.some((row) => row.notification_id === id)).toBe(true);
    expect((await journalRows(page, "replies")).filter((row) => row.reply?.notification_id === id)).toHaveLength(1);
    expect(await page.evaluate(() => (window as unknown as { __receiptOrder: string[] }).__receiptOrder.slice(0, 2))).toEqual(["reply-commit", "application-ack"]);
    await expect(page.getByRole("log").getByText("Stored on this device", { exact: true })).toBeVisible();
    await expect(page.getByRole("log")).not.toContainText("Read by operator");
    await captureReceiptSurface(page, "stored");
  });
  await journey.stage("A reload replays the same immutable reply and re-ACKs without a second bubble", async () => {
    const before = hub.persistedReplies.length;
    await page.reload(); await choose(page);
    await expect.poll(() => hub.persistedReplies.length).toBeGreaterThan(before);
    await expect(page.getByRole("log").getByText("A durable reply on this device", { exact: true })).toHaveCount(1);
    expect((await journalRows(page, "replies")).filter((row) => row.reply?.notification_id === id)).toHaveLength(1);
  });
});

test("HUB-J3 failed reply persistence withholds ACK; reload replays before storing", journeyPart, async ({ page, journey }) => {
  await page.addInitScript(() => {
    const state = window as unknown as { __failReplyStorage?: boolean };
    const transact = IDBDatabase.prototype.transaction;
    IDBDatabase.prototype.transaction = function(stores, mode, options) {
      if (state.__failReplyStorage && mode === "readwrite" && (typeof stores === "string" ? [stores] : [...stores]).includes("replies")) throw new DOMException("Injected unavailable reply storage", "QuotaExceededError");
      return transact.call(this, stores, mode, options);
    };
  });
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  let id = 0;
  await journey.stage("A forwarded reply is displayed, but failed storage grants no application ACK", async () => {
    await journey.open(); await choose(page);
    await page.evaluate(() => { (window as unknown as { __failReplyStorage?: boolean }).__failReplyStorage = true; });
    id = hub.supervisorSays(PELICAN, "Replay this if the tab disappears");
    await expect(page.getByRole("log").getByText("Forwarded · not stored on this device", { exact: true })).toBeVisible();
    expect(hub.persistedReplies.filter((row) => row.notification_id === id)).toHaveLength(0);
    expect((await journalRows(page, "replies")).filter((row) => row.reply?.notification_id === id)).toHaveLength(0);
    await captureReceiptSurface(page, "forwarded");
  });
  await journey.stage("Reload recovers the unacknowledged reply from durable hub history", async () => {
    await page.reload(); await choose(page);
    await expect.poll(() => hub.persistedReplies.some((row) => row.notification_id === id)).toBe(true);
    await expect(page.getByRole("log").getByText("Replay this if the tab disappears", { exact: true })).toHaveCount(1);
    await expect(page.getByRole("log").getByText("Stored on this device", { exact: true })).toBeVisible();
  });
});
