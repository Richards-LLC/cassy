import type { Locator, Page } from "@playwright/test";
import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN } from "./world";
import { HubDouble } from "./hub-double";
import { ProtocolClock } from "./protocol-clock";

// Every independent page must remain free of unhandled app errors too.
const pageErrors = new WeakMap<Page, string[]>();
test.beforeEach(async ({ page }) => {
  const errors: string[] = [];
  pageErrors.set(page, errors);
  page.on("pageerror", error => errors.push(error.message));
});
test.afterEach(async ({ page }) => { expect(pageErrors.get(page)).toEqual([]); });

// Keep one real scheduling/protocol smoke. Each longer outage below starts with
// a fresh page and controls BOTH the page's deadlines and the double's replies.
test("HUB-J12 switch networks without losing the conversation", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], multiplex: true });
  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await chooseConversation(page);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Live");
    expect(hub.machineSocketOpens.get("atlas")).toBe(1);
  });
  await journey.stage("The route changes under the page", async () => {
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await expect(page.locator("#conversation-connection")).toHaveText(" · Live");
    const accepted = hub.nextSend();
    await sendNow(page, "After the route change");
    expect((await accepted).text).toBe("After the route change");
    expect(hub.machineSocketOpens.get("atlas")).toBeGreaterThan(1);
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
    expect(sentTimes(hub, "After the route change")).toBe(1);
  });
});

async function chooseConversation(page: Page): Promise<void> {
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
}
async function sendNow(page: Page, text: string): Promise<void> {
  await page.getByRole("textbox", { name: "Your message" }).fill(text);
  await page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true }).click();
}
function sentTimes(hub: HubDouble, text: string): number { return hub.sends.filter(m => m.text === text).length; }

async function connected(page: Page, multiplex = true, machines = [ATLAS]) {
  const clock = new ProtocolClock(page);
  const hub = new HubDouble(page, { machines, paired: machines.map(machine => machine.id), multiplex, time: clock });
  // A repeatable retry midpoint; the real-clock smoke retains natural jitter.
  await page.addInitScript(() => { Math.random = () => 0.5; });
  await hub.install();
  await page.goto("./");
  await hub.seedPaired();
  await clock.start();
  await page.goto("./");
  await chooseConversation(page);
  const header = page.locator("#conversation-connection");
  const held = page.getByRole("log").locator(".conversation-held");
  await expect(header).toHaveText(" · Live");
  return { hub, clock, header, held };
}

// The next wire/DOM acknowledgement settles asynchronous work before another
// deadline is advanced. No readiness sleeps or host elapsed-time assertions.
async function accepted(page: Page, hub: HubDouble, text: string): Promise<void> {
  const next = hub.nextSend();
  await sendNow(page, text);
  expect((await next).text).toBe(text);
}

test("HUB-J12 network switch: delayed legacy refusal holds both sends in order", async ({ page }) => {
  await test.step("On a legacy socket, a second message sent before the refusal arrives waits too", async () => {
    const { hub, clock, header, held } = await connected(page, false);
    hub.upstreamLost(PELICAN, { refusalDelayMs: 1_500 });
    await sendNow(page, "First on legacy");
    await sendNow(page, "Second before the refusal");
    // A deliberate negative window: the wire refusal must NOT arrive early.
    await clock.advance(1_499);
    await expect(held).toHaveCount(0);
    expect(hub.sends).toHaveLength(0);
    expect(hub.deliveredRefusals).toHaveLength(0);
    await clock.advance(1);
    expect(hub.deliveredRefusals).toHaveLength(1);
    await expect(held).toHaveCount(2);
    // cas-a6f0 (journey F8): the composer names the outage the banner names.
    await expect(page.locator(".terminal-disconnected-banner .banner-text")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
    await expect(page.locator("#message-status")).toHaveText("Lost connection to Atlas · Linux. Reconnecting… Your message will go out by itself when it's back.");
    hub.upstreamBack(PELICAN);
    const next = hub.nextSend();
    await clock.advance(1_000);
    expect((await next).text).toBe("First on legacy");
    await expect(held).toHaveCount(0);
    await expect(header).toHaveText(" · Live");
    expect(hub.sends.map(m => m.text)).toEqual(["First on legacy", "Second before the refusal"]);
  });
});

test("HUB-J12 network switch: half-open machine waits for four failed heartbeats then flushes once", async ({ page }) => {
  await test.step("Tailscale goes off, then on again", async () => {
    const { hub, clock, header, held } = await connected(page);
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).locator(".conversation-preview");
    const footer = page.locator("#hub-footer-badges .machine-badge-state");
    const panel = page.locator(".status-stale").filter({ visible: true });
    await hub.down("atlas");
    for (let beat = 1; beat <= 4; beat++) {
      const failed = page.waitForEvent("requestfailed", request => request.url() === "https://atlas.test/v1/sessions");
      await clock.advance(5_000);
      await failed;
      if (beat === 1) await expect(header).toHaveText(" · Live");
      if (beat === 2) {
        // cas-a6f0 (journey F8/F9): heartbeats unanswered on a machine that
        // still reads live. Header, row, footer, Tasks panel and rail all say
        // so in one word, and a message sent now waits instead of going into a
        // dead socket.
        await expect(header).toHaveText(" · Unsteady");
        await expect(row).toHaveText("Unsteady");
        await expect(footer).toHaveText("Unsteady");
        await expect(panel).toHaveText(/^Connection unsteady — checking…/);
        await expect(page.getByText("All clear").filter({ visible: true })).toHaveCount(0);
        await expect(page.getByText(/degraded/i).filter({ visible: true })).toHaveCount(0);
        await sendNow(page, "While unsteady");
        await expect(held).toHaveText("Waiting for the connection — sends when it's back");
        await expect(page.locator("#message-status")).toHaveText("Connection to Atlas · Linux unsteady — checking… Your message will go out by itself when it's back.");
        expect(sentTimes(hub, "While unsteady")).toBe(0);
      }
      if (beat === 3) await expect(header).toHaveText(" · Unsteady");
    }
    await expect(header).toHaveText(" · Reconnecting");
    await expect(row).toHaveText("Reconnecting");
    await expect(footer).toHaveText("Reconnecting");
    await expect(page.locator(".terminal-disconnected-banner")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
    // The composer's line follows the outage from unsteady to lost.
    await expect(page.locator("#message-status")).toHaveText("Lost connection to Atlas · Linux. Reconnecting… Your message will go out by itself when it's back.");
    await sendNow(page, "While Tailscale is off");
    await expect(held).toHaveCount(2);
    await expect(held.last()).toHaveText("Waiting for the connection — sends when it's back");
    await expect(page.locator("#message-status")).toHaveText("Lost connection to Atlas · Linux. Reconnecting… Your message will go out by itself when it's back.");
    // Negative window: a held message must survive a failed reconnect unsent.
    await clock.advance(5_000);
    expect(sentTimes(hub, "While unsteady")).toBe(0);
    expect(sentTimes(hub, "While Tailscale is off")).toBe(0);
    await hub.up("atlas");
    const next = hub.nextSend();
    await clock.advance(10_000); // the promised retry ceiling, in protocol time
    expect((await next).text).toBe("While unsteady");
    await expect(header).toHaveText(" · Live");
    await expect(held).toHaveCount(0);
    await expect(page.locator("#message-status")).toBeHidden();
    expect(hub.sends.map(m => m.text)).toEqual(["While unsteady", "While Tailscale is off"]);
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
    expect(sentTimes(hub, "While unsteady")).toBe(1);
    expect(sentTimes(hub, "While Tailscale is off")).toBe(1);
  });
});

test("HUB-J12 network switch: a message sent while unsteady waits, then goes once when heartbeats answer again (cas-a6f0)", async ({ page }) => {
  await test.step("The connection wobbles and steadies without a reconnect", async () => {
    const { hub, clock, header, held } = await connected(page);
    const opens = hub.machineSocketOpens.get("atlas");
    // Two heartbeats go unanswered; the machine socket itself survives.
    const heartbeat = "https://atlas.test/v1/sessions";
    await page.route(heartbeat, route => route.abort());
    for (let beat = 1; beat <= 2; beat++) {
      const failed = page.waitForEvent("requestfailed", request => request.url() === heartbeat);
      await clock.advance(5_000);
      await failed;
    }
    await expect(header).toHaveText(" · Unsteady");
    await sendNow(page, "While it wobbles");
    await expect(held).toHaveCount(1);
    expect(hub.sends).toHaveLength(0);
    await page.unroute(heartbeat);
    const next = hub.nextSend();
    await clock.advance(5_000); // the next heartbeat, answered
    expect((await next).text).toBe("While it wobbles");
    await expect(header).toHaveText(" · Live");
    await expect(held).toHaveCount(0);
    await expect(page.locator("#message-status")).toBeHidden();
    expect(hub.machineSocketOpens.get("atlas"), "steadied, not reconnected").toBe(opens);
    // Negative window: later heartbeats must not send it again.
    await clock.advance(10_000);
    expect(sentTimes(hub, "While it wobbles")).toBe(1);
  });
});

test("HUB-J12 network switch: offline and online hints bypass the heartbeat and retry clocks", async ({ page }) => {
  await test.step("Wi-Fi hands over to cellular", async () => {
    const { hub, clock, header, held } = await connected(page);
    const before = clock.now();
    await page.context().setOffline(true);
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toHaveText(" · Reconnecting");
    await sendNow(page, "During the handover");
    await expect(held).toHaveCount(1);
    expect(hub.sends).toHaveLength(0);
    await hub.up("atlas");
    const next = hub.nextSend();
    await page.context().setOffline(false);
    expect((await next).text).toBe("During the handover");
    await expect(header).toHaveText(" · Live");
    await expect(held).toHaveCount(0);
    expect(clock.now(), "online recovers without advancing a backoff deadline").toBe(before);
    // Negative window: crossing the abandoned retry must not flush it twice.
    await clock.advance(1_000);
    expect(sentTimes(hub, "During the handover")).toBe(1);
  });
});

test("HUB-J12 network switch: waking probes a half-open socket before sending", async ({ page }) => {
  await test.step("The page wakes on a half-open socket", async () => {
    const { hub, clock, header, held } = await connected(page);
    await hub.down("atlas");
    await hub.up("atlas");
    const opens = hub.machineSocketOpens.get("atlas")!;
    const visibility = (value: "hidden" | "visible") => page.evaluate(state => {
      Object.defineProperty(document, "visibilityState", { value: state, configurable: true });
      document.dispatchEvent(new Event("visibilitychange"));
    }, value);
    await visibility("hidden");
    const heartbeat = page.waitForResponse(response => response.url() === "https://atlas.test/v1/sessions");
    await clock.advance(6_000);
    await heartbeat;
    const probe = page.waitForResponse(response => response.url() === "https://atlas.test/v1/machine");
    await visibility("visible");
    await probe;
    await sendNow(page, "Right after waking");
    await expect(held).toHaveCount(1);
    expect(hub.sends).toHaveLength(0);
    // Negative window: the unanswered probe has its full 3 s to respond.
    await clock.advance(2_999);
    expect(hub.machineSocketOpens.get("atlas")).toBe(opens);
    const next = hub.nextSend();
    await clock.advance(1);
    expect((await next).text).toBe("Right after waking");
    await expect(header).toHaveText(" · Live");
    expect(hub.machineSocketOpens.get("atlas")).toBe(opens + 1);
    await accepted(page, hub, "After waking");
    expect(hub.sends.map(m => m.text)).toEqual(["Right after waking", "After waking"]);
  });
});

test("HUB-J12 network switch: slow session attach keeps machine connected and session wording", async ({ page }) => {
  await test.step("The session's daemon link drops for a moment", async () => {
    const { hub, clock, header, held } = await connected(page);
    const banner = page.locator(".terminal-disconnected-banner");
    const footer = page.locator("#hub-footer-badges .machine-badge-state");
    await expect(footer).toHaveText("Connected");
    await page.evaluate(() => {
      const seen: string[] = [];
      (window as unknown as { __footerSeen: string[] }).__footerSeen = seen;
      const sample = () => {
        const text = document.querySelector<HTMLElement>("#hub-footer-badges .machine-badge-state")?.innerText.trim();
        if (text && seen.at(-1) !== text) seen.push(text);
      };
      new MutationObserver(sample).observe(document.body, { subtree: true, childList: true, characterData: true });
    });
    hub.upstreamLost(PELICAN);
    hub.delayAttach(PELICAN, 5_000);
    const attaches = hub.attaches.length;
    await sendNow(page, "While daemon reconnects");
    await expect(held).toHaveCount(1);
    await clock.advance(1_000);
    await hub.waitFor(() => hub.attaches.length > attaches);
    await expect(banner).toHaveText("Reconnecting to cas-src… Atlas · Linux is still connected.");
    await clock.advance(3_500); // crosses the session's 3 s state deadline
    await expect(banner).toHaveText("Reconnecting to cas-src… Atlas · Linux is still connected.");
    await expect(banner).toHaveAttribute("data-scope", "session");
    await expect(footer).toHaveText("Connected");
    await expect(page.locator("#message-status")).toHaveText("cas-src on Atlas · Linux is reconnecting. Your message will go out by itself when it's back.");
    expect(hub.sends).toHaveLength(0);
    hub.upstreamBack(PELICAN);
    const next = hub.nextSend();
    await clock.advance(1_500); // the double's delayed Welcome, on the same clock
    expect((await next).text).toBe("While daemon reconnects");
    await expect(held).toHaveCount(0);
    await expect(header).toHaveText(" · Live");
    await expect(banner).toBeHidden();
    const seen = await page.evaluate(() => (window as unknown as { __footerSeen: string[] }).__footerSeen);
    expect(seen.filter(text => text !== "Connected")).toEqual([]);
    expect(sentTimes(hub, "While daemon reconnects")).toBe(1);
  });
});

async function refusalAfter(clock: ProtocolClock, hub: HubDouble, held: Locator, delay: number): Promise<void> {
  const count = hub.upstreamRefusals.length;
  // Deliberate negative boundary: retries must respect backoff, not resend early.
  await clock.advance(delay - 1);
  expect(hub.upstreamRefusals).toHaveLength(count);
  await clock.advance(1);
  await hub.waitFor(() => hub.upstreamRefusals.length === count + 1);
  await expect(held).toHaveCount(1); // refusal processed before the next timer
}

test("HUB-J12 network switch: legacy upstream backs off and resets after uninterrupted live time", async ({ page }) => {
  const { hub, clock, header, held } = await connected(page, false);
  hub.upstreamLost(PELICAN);
  await sendNow(page, "Through repeated refusals");
  await expect(held).toHaveCount(1);
  for (const delay of [1_000, 2_000, 4_000]) await refusalAfter(clock, hub, held, delay);
  expect(hub.upstreamRefusals).toHaveLength(4);
  hub.upstreamBack(PELICAN);
  const next = hub.nextSend();
  await clock.advance(8_000);
  expect((await next).text).toBe("Through repeated refusals");
  await expect(header).toHaveText(" · Live");
  await expect(held).toHaveCount(0);
  // No delivery receipt is sent: 10 s of healthy session time resets the streak.
  await clock.advance(10_000);
  hub.upstreamLost(PELICAN);
  await sendNow(page, "After healthy session time");
  await expect(held).toHaveCount(1);
  await refusalAfter(clock, hub, held, 1_000);
  hub.upstreamBack(PELICAN);
  const retried = hub.nextSend();
  await clock.advance(2_000);
  expect((await retried).text).toBe("After healthy session time");
  await expect(held).toHaveCount(0);
  expect(hub.sends.map(m => m.text)).toEqual(["Through repeated refusals", "After healthy session time"]);
});

test("HUB-J12 network switch: a reheld send expires at its original two-minute deadline and Retry sends once", async ({ page }) => {
  const { hub, clock, header, held } = await connected(page);
  hub.upstreamLost(PELICAN);
  await sendNow(page, "While the daemon stays down");
  await expect(held).toHaveCount(1);
  // Exercise reholding, which must preserve the ORIGINAL expiry.
  for (const delay of [1_000, 2_000, 4_000]) await refusalAfter(clock, hub, held, delay);
  await clock.advance(112_999);
  await expect(held).toHaveCount(1);
  const log = page.getByRole("log");
  await expect(log.getByText("Not sent", { exact: true })).toHaveCount(0);
  await clock.advance(1);
  await expect(log.getByText("Not sent", { exact: true })).toBeVisible();
  await expect(log.getByText("The session didn't come back while it waited.")).toBeVisible();
  // With no queued send left, a healthy session attach can now settle even
  // while its daemon still refuses sends. Retry wording follows that state.
  await clock.advance(8_000);
  await expect(header).toHaveText(" · Live");
  await expect(log.locator('.bub[data-state="error"] .conversation-refused-next')).toHaveText(" Retry to send it.");
  await expect(log.getByText(/re-pair/i)).toHaveCount(0);
  await expect(held).toHaveCount(0);
  await expect(page.locator("#message-status")).not.toContainText("go out by itself");
  expect(hub.sends).toHaveLength(0);
  hub.upstreamBack(PELICAN);
  const next = hub.nextSend();
  await log.getByRole("button", { name: "Retry" }).click();
  await clock.advance(8_000);
  expect((await next).text).toBe("While the daemon stays down");
  await expect(header).toHaveText(" · Live");
  expect(sentTimes(hub, "While the daemon stays down")).toBe(1);
});

test("HUB-J12 network switch: stale proofs retry with no re-pair request", async ({ page }) => {
  await test.step("A proof refused after a switch retries on its own", async () => {
    const { hub, clock, header } = await connected(page);
    await hub.down("atlas", { sockets: "close" });
    hub.refuseProofs("atlas", 3);
    await hub.up("atlas");
    await clock.advance(1_000);
    await hub.waitFor(() => hub.refusedProofs.length >= 2);
    await clock.advance(2_000);
    await hub.waitFor(() => hub.proofRefusalsLeft("atlas") === 0);
    // Exercising the final refusal is not recovery: the machine may still
    // owe its own retry. Advance its ceiling, with IO settled at every tick.
    await clock.advance(10_000);
    await expect(header).toHaveText(" · Live");
    expect(hub.refusedProofs.map(refusal => refusal.reason)).toEqual(["stale_proof", "stale_proof", "stale_proof"]);
    await expect(page.getByText(/re-pair|needs pairing|no longer paired|was revoked/i)).toHaveCount(0);
    await accepted(page, hub, "After a refused proof");
    expect(sentTimes(hub, "After a refused proof")).toBe(1);
  });
});

test("HUB-J12 network switch: revoked pairing offers accessible Re-pair on desktop and phone", async ({ page }) => {
  await test.step("A revoked pairing says so and offers Re-pair, on a phone too", async () => {
    // Keep the original two-machine world. A singleton fleet currently leaves
    // its footer saying Reconnecting after revocation; reported separately.
    const { hub, clock, header } = await connected(page, true, [ATLAS, STUDIO]);
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await clock.advance(1_000);
    await expect(header).toHaveText(" · Needs pairing");
    await expect(page.locator(".status-stale").filter({ visible: true })).toHaveText(/^Not live — this browser needs pairing again\./);
    await expect(page.getByText(/reconnecting/i).filter({ visible: true })).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    const banner = page.locator(".terminal-disconnected-banner");
    await expect(banner.locator(".banner-text")).toHaveText("Atlas · Linux needs pairing again.");
    await expect(banner).not.toContainText("Reconnecting");
    const repair = banner.getByRole("button", { name: "Re-pair Atlas · Linux" });
    await expect(repair).toBeVisible();
    expect((await repair.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    await repair.click();
    await expect(page.locator("#pair-dialog")).toBeVisible();
  });
});

test("HUB-J12 network switch: a revoked pairing settles waiting sends and is named one way everywhere (cas-a6f0)", async ({ page }) => {
  await test.step("The pairing is revoked while messages wait", async () => {
    const { hub, clock, header, held } = await connected(page);
    const log = page.getByRole("log");
    // On the wire, with no receipt yet.
    await accepted(page, hub, "On the wire before the revoke");
    await expect(log.getByText("Sending…")).toHaveCount(1);
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toHaveText(" · Reconnecting");
    await sendNow(page, "Held before the revoke");
    await expect(held).toHaveCount(1);
    await hub.up("atlas");
    await clock.advance(1_000);
    await expect(header).toHaveText(" · Needs pairing");
    // Nothing keeps saying it is sending (journey F35).
    await expect(log.getByText("Sending…")).toHaveCount(0);
    await expect(held).toHaveCount(0);
    const refused = log.locator('.bub[data-state="error"]');
    await expect(refused).toHaveCount(1);
    await expect(refused).toContainText("Held before the revoke");
    await expect(refused.locator(".conversation-refused-reason")).toHaveText("Atlas · Linux needs pairing again. Re-pair Atlas · Linux, then retry.");
    await expect(refused.getByRole("button", { name: "Retry sending" })).toBeVisible();
    await expect(log.locator('.bub[data-state="unconfirmed"]')).toContainText("On the wire before the revoke");
    await expect(page.locator("#message-status")).toHaveText("Not sent — see the message above.");
    // The rail card is headed as the header and banner word it (journey F8).
    await expect(page.getByText("Machine needs pairing").filter({ visible: true }).first()).toBeVisible();
    await expect(page.getByText(/Authentication blocked/).filter({ visible: true })).toHaveCount(0);
    await expect(page.getByText(/reconnecting/i).filter({ visible: true })).toHaveCount(0);
    // Negative window: the held send never goes by itself.
    await clock.advance(10_000);
    expect(sentTimes(hub, "Held before the revoke")).toBe(0);
  });
});

// cas-f698: the old two-machine journey hid this behind the healthy STUDIO.
// Exact failure: footer .machine-badge-state still says "Reconnecting" after
// ATLAS alone reaches Needs pairing; the footer must use the same machine words.
test("HUB-J12 network switch: single revoked machine stops promising reconnection (cas-f698)", async ({ page }) => {
  const { hub, clock, header } = await connected(page);
  hub.refuseProofs("atlas", 1_000, "revoked", false);
  await hub.down("atlas", { sockets: "close" });
  await hub.up("atlas");
  await clock.advance(1_000);
  await expect(header).toHaveText(" · Needs pairing");
  await expect(page.locator("#hub-footer-badges .machine-badge-state")).toHaveText("Needs pairing");
});
