import { test, expect, journeyPart } from "./journey";
import { HubDouble } from "./hub-double";
import { ATLAS, PELICAN, STUDIO } from "./world";
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

test("HUB-J12 a receipt in either tab settles both tabs without a duplicate retry (cas-9dc6)", journeyPart, async ({ page, context, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  const peer = await context.newPage();
  await peer.clock.install({ time: journeyNow() });
  const other = new HubDouble(peer, { machines: [ATLAS], paired: ["atlas"], multiplex: true });
  await other.install();
  await journey.stage("Both tabs keep the same offline message", async () => {
    await journey.open(); await choose(page);
    await peer.goto(page.url()); await choose(peer);
    await Promise.all([hub.down("atlas", { sockets: "close" }), other.down("atlas", { sockets: "close" })]);
    for (const tab of [page, peer]) await expect(tab.locator("#conversation-connection")).not.toHaveText(" · Live");
    await send(page, "One instruction, one delivery");
    for (const tab of [page, peer]) await expect(tab.getByRole("log")).toContainText("One instruction, one delivery");
  });
  await journey.stage("One tab sends; its confirmation is visible in both tabs", async () => {
    await Promise.all([hub.up("atlas"), other.up("atlas")]);
    await expect.poll(() => hub.sends.length + other.sends.length, { timeout: 20_000 }).toBe(1);
    const owner = hub.sends.length ? hub : other;
    owner.deliverLatest(PELICAN);
    for (const tab of [page, peer]) {
      await expect(tab.getByRole("log")).toContainText("One instruction, one delivery");
      await expect(tab.getByRole("log").locator(".conversation-delivery")).toContainText("Delivered");
      await expect(tab.getByRole("button", { name: "Retry sending", exact: true })).toHaveCount(0);
    }
    expect(hub.sends.length + other.sends.length).toBe(1);
  });
  await peer.close();
});

test("HUB-J12 two tabs can explicitly retry an unconfirmed send with its original reference (cas-9dc6 F01)", journeyPart, async ({ page, context, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  const peer = await context.newPage();
  await peer.clock.install({ time: journeyNow() });
  const other = new HubDouble(peer, { machines: [ATLAS], paired: ["atlas"], multiplex: true });
  await other.install();
  await journey.stage("Both live tabs see a send whose receipt has not arrived", async () => {
    await journey.open(); await choose(page);
    await peer.goto(page.url()); await choose(peer);
    await send(page, "Retry this same instruction");
    await expect.poll(() => hub.sends.length + other.sends.length).toBe(1);
    for (const tab of [page, peer]) {
      await expect(tab.getByRole("button", { name: "Retry sending", exact: true })).toBeVisible({ timeout: 20_000 });
    }
  });
  await journey.stage("Explicit Retry crosses the wire without waiting for a reconnect", async () => {
    await page.getByRole("button", { name: "Retry sending", exact: true }).click();
    await expect.poll(() => hub.sends.length + other.sends.length).toBe(2);
    expect(new Set([...hub.sends, ...other.sends].map(send => send.client_ref)).size).toBe(1);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Live");
  });
  await journey.stage("A late receipt settles both tabs and removes Retry", async () => {
    (hub.sends.length ? hub : other).deliverLatest(PELICAN);
    for (const tab of [page, peer]) {
      await expect(tab.getByRole("log").locator(".conversation-delivery")).toContainText("Delivered");
      await expect(tab.getByRole("button", { name: "Retry sending", exact: true })).toHaveCount(0);
    }
    expect(hub.sends.length + other.sends.length).toBe(2);
  });
  await peer.close();
});

async function captureReceiptSurface(page: Page, state: "stored" | "forwarded") {
  const qa = process.env.DELIVERY_QA;
  if (!qa) return;
  await mkdir(qa, { recursive: true });
  const viewport = page.viewportSize()!;
  for (const [size, width, height] of [["desktop", 1280, 800], ["phone", 390, 844]] as const) {
    for (const colorScheme of ["light", "dark"] as const) {
      await page.setViewportSize({ width, height }); await page.emulateMedia({ colorScheme });
      // cas-97d58 F05: a kept reply shows nothing; only an unkept one says so.
      if (state === "stored") await expect(page.getByRole("log").locator('[data-stored="true"]').first()).toBeVisible();
      else await expect(page.getByRole("log")).toContainText("Not kept on this device yet");
      await page.screenshot({ path: join(qa, `${state}-${colorScheme}-${size}.png`) });
    }
  }
  const html = await page.locator(".conversation-reading").evaluate(node => node.outerHTML);
  const css = await readFile("dist/app.css", "utf8");
  await writeFile(join(qa, `${state}.html`), `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>${css}</style></head><body>${html}</body></html>`);
  for (const [name, query, media] of [["forced-colors", "(forced-colors: active)", { forcedColors: "active" }], ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }], ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }]] as const) {
    await page.emulateMedia(Object.assign({ forcedColors: null, reducedMotion: null, contrast: null }, media));
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
  const references = (await journalRows(page, "sends")).filter(row => row.send?.state === "held").map(row => row.send!.id).sort();
  await journey.stage("Both tabs reload the same two held client references", async () => {
    await Promise.all([page.reload(), second.reload()]);
    // A wholly unreachable hub has no live session catalog on cold reload.
    // Verify kept references on both devices before either transport recovers.
    for (const tab of [page, second]) {
      expect((await journalRows(tab, "sends")).filter(row => row.send?.state === "held").map(row => row.send!.id).sort()).toEqual(references);
    }
    expect([...hub.sends, ...other.sends]).toHaveLength(0);
  });
  await journey.stage("Both connections recover; each item crosses the wire once", async () => {
    await Promise.all([hub.up("atlas"), other.up("atlas")]);
    await Promise.all([choose(page), choose(second)]);
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

test("HUB-J12 send while offline, reconnect, watch it deliver: the waiting line clears (cas-387e)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  const status = page.locator("#message-status");
  await journey.stage("Send while the machine is away: the message waits, and the composer says so", async () => {
    await journey.open(); await choose(page);
    await hub.down("atlas", { sockets: "close" });
    await expect(page.locator("#conversation-connection")).not.toHaveText(" · Live");
    await send(page, "Send this when Atlas is back");
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(1);
    await expect(status).toHaveText("Lost connection to Atlas · Linux. Reconnecting… Your message will go out by itself when it's back.");
  });
  await journey.stage("Atlas comes back: the message goes out once and the waiting line clears", async () => {
    await hub.up("atlas");
    await choose(page);
    await expect.poll(() => hub.sends.map((row) => row.text), { timeout: 20_000 }).toEqual(["Send this when Atlas is back"]);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Live");
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(0);
    await expect(status).toBeHidden();
  });
  await journey.stage("Delivered: nothing says it will go out by itself", async () => {
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
    await expect(status).toBeHidden();
    await expect(page.getByText(/go out by itself/)).toHaveCount(0);
    expect(hub.sends.map((row) => row.text)).toEqual(["Send this when Atlas is back"]);
  });
  const qa = process.env.QA_ARTIFACTS;
  if (qa) {
    // cas-qa-craft polish evidence for cas-387e: the delivered thread and
    // composer at 1280 and 390, light and dark; a standalone snapshot with
    // the committed CSS; the three a11y modes proven by matchMedia.
    await mkdir(qa, { recursive: true });
    for (const [size, width, height] of [["desktop", 1280, 800], ["phone", 390, 844]] as const) {
      for (const colorScheme of ["light", "dark"] as const) {
        await page.setViewportSize({ width, height }); await page.emulateMedia({ colorScheme });
        await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
        await expect(status).toBeHidden();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scroll").toBe(true);
        await page.screenshot({ path: join(qa, `delivered-${colorScheme}-${size}.png`) });
      }
    }
    await page.setViewportSize({ width: 1280, height: 800 }); await page.emulateMedia({ colorScheme: "light" });
    // The thread and its composer together: the composer's status line is the subject.
    const html = await page.evaluate(() => {
      const thread = document.querySelector(".conversation-reading")!;
      let node: Element | null = document.querySelector("#message-status")!.parentElement;
      while (node && !node.contains(thread)) node = node.parentElement;
      return node!.outerHTML;
    });
    const css = await readFile("dist/app.css", "utf8");
    await writeFile(join(qa, "delivered.html"), `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Delivered conversation</title><style>${css}</style></head><body>${html}</body></html>`);
    for (const [name, query, media] of [["forced-colors", "(forced-colors: active)", { forcedColors: "active" }], ["reduced-motion", "(prefers-reduced-motion: reduce)", { reducedMotion: "reduce" }], ["contrast-more", "(prefers-contrast: more)", { contrast: "more" }]] as const) {
      await page.emulateMedia(Object.assign({ forcedColors: null, reducedMotion: null, contrast: null }, media));
      expect(await page.evaluate(query => matchMedia(query).matches, query)).toBe(true);
      await expect(status).toBeHidden();
      await page.screenshot({ path: join(qa, `a11y-${name}.png`) });
    }
    await page.emulateMedia({ forcedColors: null, reducedMotion: null, contrast: null, colorScheme: "light" });
  }
});

test("HUB-J12 a held message another tab delivers clears this tab's waiting line (cas-387e)", journeyPart, async ({ page, context, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  const second = await context.newPage();
  await second.clock.install({ time: journeyNow() });
  const other = new HubDouble(second, { machines: [ATLAS], paired: ["atlas"], multiplex: true });
  await other.install();
  const status = page.locator("#message-status");
  await journey.stage("This tab holds a message while the machine is away", async () => {
    await journey.open(); await choose(page);
    await second.goto(page.url()); await choose(second);
    await Promise.all([hub.down("atlas", { sockets: "close" }), other.down("atlas", { sockets: "close" })]);
    await expect(page.locator("#conversation-connection")).not.toHaveText(" · Live");
    await send(page, "Held in the first tab");
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(1);
    await expect(status).toContainText("will go out by itself");
  });
  await journey.stage("The other tab reconnects first and delivers it; this tab stops promising", async () => {
    await other.up("atlas");
    await choose(second);
    await expect.poll(() => other.sends.map((row) => row.text), { timeout: 20_000 }).toEqual(["Held in the first tab"]);
    other.deliverLatest(PELICAN);
    await expect(second.getByRole("log").getByText("Delivered")).toBeVisible();
    await expect(second.locator("#message-status")).toBeHidden();
    // Nothing is held for this conversation any more, so the first tab no
    // longer says the message will go out by itself, even while still offline.
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(0);
    await expect(status).toBeHidden();
  });
  await journey.stage("This tab comes back: no waiting line, sent once", async () => {
    await hub.up("atlas");
    await choose(page);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Live");
    // Never a second copy of it here; the composer stays clear.
    expect(await page.getByRole("log").getByText("Held in the first tab").count()).toBeLessThanOrEqual(1);
    await expect(status).toBeHidden();
    expect([...hub.sends, ...other.sends].map((row) => row.text)).toEqual(["Held in the first tab"]);
  });
  await second.close();
});

test("HUB-J12 cancellation persists and cannot drain after reload", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  await journey.stage("Cancel an explicitly waiting message", async () => {
    await journey.open(); await choose(page);
    hub.upstreamLost(PELICAN);
    await send(page, "Cancel this waiting instruction");
    const cancel = page.getByRole("button", { name: "Cancel waiting message" });
    // cas-97d58 F15: the label and outline take the bubble's own ink, so they
    // read on the navy bubble instead of near-black on navy (about 1.8:1).
    const ink = await cancel.evaluate((button) => {
      const bubble = button.closest(".bub")!;
      return { label: getComputedStyle(button).color, border: getComputedStyle(button).borderTopColor, bubble: getComputedStyle(bubble).color };
    });
    expect(ink.label).toBe(ink.bubble);
    expect(ink.border).toBe(ink.bubble);
    await cancel.click();
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(0);
    // A cancel is the operator's choice, not a failure: no red "unsent" chip.
    await expect(page.getByText("Cancel this waiting instruction", { exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: /unsent message/ })).toHaveCount(0);
    await expect(page.locator("#message-status")).toHaveText("Waiting message cancelled. It was not sent.");
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
    // cas-97d58 F05 (supersedes cas-e6d2's always-visible receipt): kept is the
    // quiet normal state, recorded on the bubble, never read out after every turn.
    await expect(page.getByRole("log").locator('[data-stored="true"]').filter({ hasText: "A durable reply on this device" })).toHaveCount(1);
    await expect(page.getByRole("log").getByText(/Stored on this device|Not kept on this device yet/)).toHaveCount(0);
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
    await expect(page.getByRole("log").getByText("Not kept on this device yet", { exact: true })).toBeVisible();
    await expect(page.getByRole("log").locator('[data-stored="false"]').filter({ hasText: "Replay this if the tab disappears" })).toHaveCount(1);
    expect(hub.persistedReplies.filter((row) => row.notification_id === id)).toHaveLength(0);
    expect((await journalRows(page, "replies")).filter((row) => row.reply?.notification_id === id)).toHaveLength(0);
    await captureReceiptSurface(page, "forwarded");
  });
  await journey.stage("Reload recovers the unacknowledged reply from durable hub history", async () => {
    await page.reload(); await choose(page);
    await expect.poll(() => hub.persistedReplies.some((row) => row.notification_id === id)).toBe(true);
    await expect(page.getByRole("log").getByText("Replay this if the tab disappears", { exact: true })).toHaveCount(1);
    await expect(page.getByRole("log").locator('[data-stored="true"]').filter({ hasText: "Replay this if the tab disappears" })).toHaveCount(1);
    await expect(page.getByRole("log").getByText("Not kept on this device yet")).toHaveCount(0);
  });
});

test("HUB-J3 replies read before a reload stay read after it (cas-97d58 F14)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const casSrc = list.getByRole("button", { name: /cas-src/ });
  await journey.stage("Read a reply, then move to another conversation", async () => {
    await journey.open();
    await casSrc.click();
    hub.supervisorSays(PELICAN, "Read before the reload.");
    await expect(page.getByRole("log").getByText("Read before the reload.")).toBeVisible();
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true })).toBeVisible();
    await expect(casSrc.getByLabel(/unread/)).toHaveCount(0);
  });
  await journey.stage("A reload replays the history; what was read is still read", async () => {
    await page.reload();
    await expect(casSrc).toBeVisible({ timeout: 15_000 });
    await expect(casSrc).toContainText("Read before the reload.");
    await expect(casSrc.getByLabel(/unread/)).toHaveCount(0);
  });
});
