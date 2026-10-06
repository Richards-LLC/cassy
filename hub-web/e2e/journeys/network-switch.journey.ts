import type { Locator, Page } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";
import { HubDouble, SCOPES } from "./hub-double";
import { ProtocolClock } from "./protocol-clock";
import { journalRows } from "./commander-journal-storage";

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
  await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
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

test("HUB-J12 network switch: delayed legacy refusal holds both sends in order", journeyPart, async ({ page, journey }) => {
  await journey.stage("On a legacy socket, a second message sent before the refusal arrives waits too", async () => {
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
    // cas-387e: the session's link never dropped, so nothing reattached; the
    // composer's "will go out by itself" line still clears once the held
    // messages went out, and stays clear once they are Delivered.
    await expect(page.locator("#message-status")).toBeHidden();
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
    await expect(page.locator("#message-status")).toBeHidden();
  });
});

test("HUB-J12 network switch: half-open machine waits for four failed heartbeats then flushes once", journeyPart, async ({ page, journey }) => {
  await journey.stage("Tailscale goes off, then on again", async () => {
    const { hub, clock, header, held } = await connected(page);
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).locator(".conversation-preview");
    const footer = page.locator("#hub-footer-badges .machine-badge-state");
    const panel = page.locator(".status-stale").filter({ visible: true });
    await hub.down("atlas");
    for (let beat = 1; beat <= 4; beat++) {
      const failed = page.waitForEvent("requestfailed", request => request.url() === "https://atlas.test/v1/sessions");
      await clock.advance(5_000);
      await failed;
      if (beat === 1) {
        await expect(header).toHaveText(" · Live");
        // cas-eefe: Chromium can revise throughput/RTT estimates under load
        // without changing the route. That hint cannot promote the next
        // failed heartbeat from Unsteady straight to Reconnecting.
        await page.evaluate(() => {
          const connection = (navigator as Navigator & { connection?: EventTarget }).connection;
          if (!connection) throw new Error("Chromium NetworkInformation is required for this regression");
          for (const [name, value] of Object.entries({ rtt: 250, downlink: 1, effectiveType: "3g" })) {
            Object.defineProperty(connection, name, { value, configurable: true });
          }
          connection.dispatchEvent(new Event("change"));
        });
      }
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
        await expect(page.getByText(/degraded|connection dropped/i).filter({ visible: true })).toHaveCount(0);
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
    await expect(page.locator(".terminal-disconnected-banner .banner-text")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
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

test("HUB-J12 network switch: a message sent while unsteady waits, then goes once when heartbeats answer again (cas-a6f0)", journeyPart, async ({ page, journey }) => {
  await journey.stage("The connection wobbles and steadies without a reconnect", async () => {
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

test("HUB-J12 network switch: offline and online hints bypass the heartbeat and retry clocks", journeyPart, async ({ page, journey }) => {
  await journey.stage("Wi-Fi hands over to cellular", async () => {
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

test("HUB-J12 network switch: waking probes a half-open socket before sending", journeyPart, async ({ page, journey }) => {
  await journey.stage("The page wakes on a half-open socket", async () => {
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

test("HUB-J12 network switch: slow session attach keeps machine connected and session wording", journeyPart, async ({ page, journey }) => {
  await journey.stage("The session's daemon link drops for a moment", async () => {
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
    // The sentence is the banner's live words; its Details button sits beside them (cas-2b3a5).
    await expect(banner.locator(".banner-text")).toHaveText("Reconnecting to cas-src… Atlas · Linux is still connected.");
    await clock.advance(3_500); // crosses the session's 3 s state deadline
    await expect(banner.locator(".banner-text")).toHaveText("Reconnecting to cas-src… Atlas · Linux is still connected.");
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

test("HUB-J12 network switch: legacy upstream backs off and resets after uninterrupted live time", journeyPart, async ({ page, journey }) => {
  await journey.stage("Once the session has stayed live for 10 s, the next drop retries after about 1 s again", async () => {
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
});

test("HUB-J12 network switch: a reheld send expires at its original two-minute deadline and Retry sends once", journeyPart, async ({ page, journey }) => {
  await journey.stage("Past the two-minute hold, the message reads Not sent with Retry, and Retry sends it once", async () => {
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
});

test("HUB-J12 network switch: stale proofs retry with no re-pair request", journeyPart, async ({ page, journey }) => {
  await journey.stage("A proof refused after a switch retries on its own", async () => {
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

test("HUB-J12 network switch: revoked pairing offers accessible Re-pair on desktop and phone", journeyPart, async ({ page, journey }) => {
  await journey.stage("A revoked pairing says so and offers Re-pair, on a phone too", async () => {
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

test("HUB-J12 network switch: a revoked pairing settles waiting sends and is named one way everywhere (cas-a6f0)", journeyPart, async ({ page, journey }) => {
  await journey.stage("The pairing is revoked while messages wait", async () => {
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
    // Negative window: the held send never goes by itself, and the refusal
    // stands through later heartbeats (cas-05c0 QA).
    await clock.advance(10_000);
    expect(sentTimes(hub, "Held before the revoke")).toBe(0);
    await expect(header).toHaveText(" · Needs pairing");
    await expect(page.getByText("Machine needs pairing").filter({ visible: true }).first()).toBeVisible();
    await expect(page.locator(".status-stale").filter({ visible: true })).toHaveText(/^Not live — this browser needs pairing again\./);
    await expect(page.getByText(/reconnecting/i).filter({ visible: true })).toHaveCount(0);
  });
});

test("HUB-J12 network switch: a machine drop is told by the banner alone, and control comes back with the machine (cas-7b31)", journeyPart, async ({ page, journey }) => {
  await journey.stage("The whole machine drops, then returns", async () => {
    const { hub, clock, header } = await connected(page);
    const rail = page.locator("#attention-panel");
    const transport = /Reconnecting to hub|Hub connection lost|Stuck dialing|heartbeats missed|attach failed|needs attention/i;
    const leaseTakes: string[] = [];
    page.on("request", request => { if (request.method() === "POST" && request.url().endsWith("/lease")) leaseTakes.push(request.url()); });
    await hub.down("atlas", { sockets: "close" });
    await expect(header).toHaveText(" · Reconnecting");
    await expect(page.locator(".terminal-disconnected-banner .banner-text")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
    // The header's Interrupt waits, and says why in the banner's words (cas-0546).
    const interrupt = page.getByRole("button", { name: "Interrupt the cas-src supervisor", exact: true });
    await expect(interrupt).toHaveAttribute("aria-disabled", "true");
    await expect(interrupt).toHaveAccessibleDescription("Lost connection to Atlas · Linux. Interrupt and raw output return when it reconnects.");
    // Retrying: no rail card beside the banner, through several retries (journey F1).
    for (let attempt = 0; attempt < 3; attempt++) {
      await clock.advance(5_000);
      await expect(rail.getByText(transport)).toHaveCount(0);
    }
    // The conversation view raises no control toast over the thread (journey F2).
    await expect(page.getByText(/Control released/)).toHaveCount(0);
    expect(leaseTakes).toEqual([]);
    await hub.up("atlas");
    await clock.advance(10_000);
    await expect(header).toHaveText(" · Live");
    // Control held before the drop is taken back, nothing having been sent.
    await expect.poll(() => leaseTakes.length).toBe(1);
    await expect(rail.getByText(transport)).toHaveCount(0);
    // cas-ca7f: no "Control released" notice is left on screen or in the
    // accessibility tree once control is back, and Interrupt is available
    // again in the conversation header (cas-0546).
    expect(await page.locator("body").ariaSnapshot()).not.toContain("Control released");
    await expect(interrupt).not.toHaveAttribute("aria-disabled", "true");
    await expect(interrupt).toHaveAccessibleDescription("");
  });
});

test("HUB-J12 network switch: a refused pairing leaves no 'connection dropped' notice, and Interrupt says to re-pair (cas-7b31)", journeyPart, async ({ page, journey }) => {
  await journey.stage("A refused pairing leaves no connection-dropped notice, and Interrupt says to re-pair", async () => {
    const { hub, clock } = await connected(page, true);
    const interrupt = page.getByRole("button", { name: "Interrupt the cas-src supervisor", exact: true });
    const raw = page.getByRole("button", { name: "Raw output", exact: true });
    await expect(interrupt).not.toHaveAttribute("aria-disabled", "true");
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await clock.advance(1_000);
    await expect(page.locator(".terminal-disconnected-banner .banner-text")).toHaveText("Atlas · Linux needs pairing again.");
    await expect(page.getByText(/connection dropped/i)).toHaveCount(0);
    expect(await page.locator("body").ariaSnapshot()).not.toContain("connection dropped");
    // Nor does control come back by itself: the pairing must be repaired
    // first, and the conversation's actions say so instead of promising a
    // reconnect (cas-0546).
    const repair = "Atlas · Linux needs pairing again. Re-pair it to interrupt the supervisor or read its raw output.";
    for (const action of [interrupt, raw]) {
      await expect(action).toHaveAttribute("aria-disabled", "true");
      await expect(action).toHaveAccessibleDescription(repair);
    }
    // Heartbeats keep ticking: the refusal must stand on every surface (it
    // turned "live" again on the next beat, cas-05c0 QA).
    for (let beat = 0; beat < 3; beat++) await clock.advance(5_000);
    for (const action of [interrupt, raw]) await expect(action).toHaveAccessibleDescription(repair);
    await expect(page.locator(".terminal-disconnected-banner .banner-text")).toHaveText("Atlas · Linux needs pairing again.");
    await expect(page.locator("#conversation-connection")).toHaveText(" · Needs pairing");
    await expect(page.getByText(/return when it reconnects|Reconnecting/).filter({ visible: true })).toHaveCount(0);
    // A press says why instead of interrupting.
    await interrupt.dispatchEvent("click");
    await expect(page.locator("#toast")).toHaveText(repair);
    expect(hub.frames.filter((frame) => frame.kind === "InterruptPane")).toEqual([]);
  });
});

// cas-f698: the old two-machine journey hid this behind the healthy STUDIO.
// Exact failure: footer .machine-badge-state still says "Reconnecting" after
// ATLAS alone reaches Needs pairing; the footer must use the same machine words.
test("HUB-J12 network switch: single revoked machine stops promising reconnection (cas-f698)", journeyPart, async ({ page, journey }) => {
  await journey.stage("A single revoked machine stops promising reconnection: Needs pairing, and no message reads working", async () => {
    const { hub, clock, header } = await connected(page);
    // cas-5a8f: a send the supervisor has, awaiting its reply, reads "working"
    // while the machine is live...
    await accepted(page, hub, "Before the pairing is revoked");
    // cas-71f4: the supervisor has it once it is delivered; until then the
    // bubble's "Sending…" is the one signal.
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
    const working = page.getByRole("log").locator(".working");
    await expect(working).toHaveCount(1);
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await clock.advance(1_000);
    await expect(header).toHaveText(" · Needs pairing");
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).toHaveText("Needs pairing");
    // ...and not beside "Needs pairing": the page can no longer know, on screen
    // or to a screen reader.
    await expect(working).toHaveCount(0);
    expect(await page.getByRole("log").ariaSnapshot()).not.toContain("status: working");
  });
});

// cas-460a: a pairing revoked while Paired machines is open rebuilds the shell
// under the dialog. Escape must still hand focus back to the footer control
// that opened it, not to the page; Enter on that control opens it again.
test("HUB-J12 network switch: Paired machines returns focus to its footer opener across a revoked pairing (cas-460a)", journeyPart, async ({ page, journey }) => {
  const footer = page.locator("#paired-machines-toggle");
  const dialog = page.getByRole("dialog", { name: "Paired machines" });
  let revoke: () => Promise<void> = async () => {};
  await journey.stage("Open Paired machines from the footer by keyboard while the machine is live", async () => {
    const { hub, clock, header } = await connected(page);
    revoke = async () => {
      hub.refuseProofs("atlas", 1_000, "revoked", false);
      await hub.down("atlas", { sockets: "close" });
      await hub.up("atlas");
      await clock.advance(1_000);
      await expect(header).toHaveText(" · Needs pairing");
    };
    await footer.focus();
    await expect(footer).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
  });
  await journey.stage("The pairing is revoked while it is open; Escape returns to the footer", async () => {
    await revoke();
    await expect(dialog.locator(".paired-machine-state").first()).toContainText("Needs pairing");
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(page.locator("#paired-machines-toggle")).toBeFocused();
  });
  await journey.stage("Enter on the footer opens it again, and Escape returns there again", async () => {
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).toHaveText("Needs pairing");
    await page.keyboard.press("Enter");
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(page.locator("#paired-machines-toggle")).toBeFocused();
  });
});

// cas-7752: drafts are kept on disk per conversation, so a revoked pairing
// must take them with it — and the renders after it must not write them back.
test("HUB-J12 network switch: a revoked pairing leaves no stored draft behind (cas-7752)", journeyPart, async ({ page, journey }) => {
  await journey.stage("A revoked pairing takes its stored draft with it, and nothing typed after is stored", async () => {
    const { hub, clock, header } = await connected(page);
    const stored = () => page.evaluate(() => localStorage.getItem("cas-commander-conversation:drafts:v1") ?? "");
    await page.getByRole("textbox", { name: "Your message" }).fill("A private draft for Atlas");
    await expect.poll(stored, { message: "the draft is stored while the pairing is good" }).toContain("A private draft for Atlas");
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await clock.advance(1_000);
    await expect(header).toHaveText(" · Needs pairing");
    // Several heartbeat renders later, it has not been written back.
    await clock.advance(15_000);
    expect(await stored()).not.toContain("A private draft for Atlas");
    // Nor is anything typed after the revoke, though it stays on screen.
    await page.getByRole("textbox", { name: "Your message" }).pressSequentially(" and more");
    await expect(page.getByRole("textbox", { name: "Your message" })).toHaveValue("A private draft for Atlas and more");
    await clock.advance(15_000);
    expect(await stored()).not.toContain("atlas:");
    // cas-b452 (journey F37): the list and the open conversation agree. Long
    // past the catalog's freshness, the open conversation's row is still
    // listed, open and marked Needs pairing; the list never empties beside it.
    const list = page.getByRole("navigation", { name: "Choose a supervisor" });
    const row = list.getByRole("button", { name: /cas-src/ });
    await expect(row).toBeVisible();
    await expect(row).toHaveAttribute("aria-current", "true");
    await expect(row).toContainText("Needs pairing");
    await expect(page.locator("#conversation-empty")).toBeHidden();
    await expect(page.locator(".hub-footer-meta")).toContainText("1 conversation");
  });
});

// cas-e7b1: a phone that discards the tab during a network switch reloads it.
// The operator's unsettled messages come back as what they are: one still
// waiting goes out once when the session is back; one whose receipt never
// came stays "Not confirmed" and is not sent again by itself.
test("HUB-J12 network switch: a waiting and a not-confirmed message survive a reload, and the waiting one goes once (cas-e7b1)", journeyPart, async ({ page, journey }) => {
  const { hub, clock, held } = await connected(page);
  const log = page.getByRole("log");
  const bubble = (text: string) => log.locator(".bub").filter({ hasText: text });
  const stored = async () => JSON.stringify(await journalRows(page, "sends"));
  await journey.stage("One message goes out and its receipt never comes; the next waits for the session", async () => {
    const first = hub.nextSend();
    await sendNow(page, "Did this one land?");
    expect((await first).text).toBe("Did this one land?");
    await clock.advance(15_000); // the receipt deadline
    await expect(bubble("Did this one land?")).toContainText("Not confirmed");
    hub.upstreamLost(PELICAN);
    await sendNow(page, "Send this when it's back");
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    expect(sentTimes(hub, "Send this when it's back")).toBe(0);
  });
  await journey.stage("The tab reloads: both are still there, saying what they are", async () => {
    await page.reload();
    await chooseConversation(page);
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    await expect(bubble("Send this when it's back")).not.toContainText(/Sending…|Delivered/);
    await expect(bubble("Did this one land?")).toContainText("Not confirmed");
    await expect(bubble("Did this one land?")).not.toContainText(/Sending…|Delivered/);
    expect(sentTimes(hub, "Send this when it's back")).toBe(0);
  });
  await journey.stage("The session is back: the waiting one goes out once, the other is not resent", async () => {
    hub.upstreamBack(PELICAN);
    const next = hub.nextSend();
    await clock.advance(10_000); // the reattach retry ceiling, in protocol time
    expect((await next).text).toBe("Send this when it's back");
    await expect(held).toHaveCount(0);
    hub.deliverLatest(PELICAN);
    await expect(bubble("Send this when it's back")).toContainText("Delivered");
    await clock.advance(30_000);
    expect(sentTimes(hub, "Send this when it's back")).toBe(1);
    expect(sentTimes(hub, "Did this one land?")).toBe(1);
    // Delivered, it is no longer kept; the unconfirmed one still is.
    expect(await stored()).not.toContain("Send this when it's back");
    expect(await stored()).toContain("Did this one land?");
  });
  await journey.stage("The pairing is revoked: the kept message leaves the disk with it, and stays on screen", async () => {
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await clock.advance(1_000);
    await expect(page.locator("#conversation-connection")).toHaveText(" · Needs pairing");
    await expect(bubble("Did this one land?")).toContainText("Not confirmed");
    await clock.advance(15_000);
    expect(await stored()).not.toContain('"hub":"atlas"');
  });
});

// cas-0e14 (journey F29): a code re-pair requests only the default scopes, so
// a browser that could start sessions loses that. The dialog says so before
// the code is made, with the command whose link would keep it, and once the
// new credential is in, Attention says starting sessions needs allowing again
// and New session leads with it until it is allowed.
test("HUB-J12 network switch: re-pairing by code says plainly that starting sessions must be allowed again (cas-0e14)", journeyPart, async ({ page, journey }) => {
  test.setTimeout(120_000);
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], scopes: { atlas: [...SCOPES, "session-launch"] }, relay: { machine: "atlas", claimAfter: 1, authorizeAfter: 2 } });
  const dialog = page.locator("#pair-dialog");
  const header = page.locator("#conversation-connection");

  await journey.stage("A pairing that could start sessions is revoked", async () => {
    await journey.open();
    await chooseConversation(page);
    await expect(page.getByRole("button", { name: "New session", exact: true })).toBeVisible();
    hub.refuseProofs("atlas", 1_000, "revoked", false);
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await expect(header).toHaveText(" · Needs pairing", { timeout: 30_000 });
  });

  await journey.stage("Re-pair says starting sessions won't come with a code, and how to keep it", async () => {
    await page.getByRole("button", { name: "Re-pair Atlas · Linux" }).filter({ visible: true }).first().click();
    await expect(dialog).toBeVisible();
    await expect(dialog.locator(".pair-status")).toContainText("starting sessions will need to be allowed again");
    // cas-093d F02: the command is its own code token with Copy, not prose
    // that broke mid-token when it wrapped.
    const command = "cas hub pair --origin " + new URL(page.url()).origin + " --scopes machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt,session:launch";
    await expect(dialog.locator(".pair-status")).not.toContainText("cas hub pair");
    await expect(dialog.locator(".pair-command code")).toHaveText(command);
    // No line break falls between two non-space characters, at any width from
    // a phone to a wide desktop, before and after Copy relabels the button
    // (cas-093d QA F01: "--" / "scopes" at 450–470 px, and 530–640 px once it
    // read "Copied"). Measured per character, so it holds for any markup.
    const splitWords = () => dialog.locator(".pair-command code").evaluate((code) => {
      const chars: Array<{ char: string; top: number }> = [];
      const walker = document.createTreeWalker(code, NodeFilter.SHOW_TEXT);
      const range = document.createRange();
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        const text = node.textContent ?? "";
        for (let index = 0; index < text.length; index += 1) {
          range.setStart(node, index); range.setEnd(node, index + 1);
          chars.push({ char: text[index]!, top: Math.round(range.getClientRects()[0]?.top ?? 0) });
        }
      }
      return chars.flatMap((item, index) => index > 0 && item.top !== chars[index - 1]!.top && item.char !== " " && chars[index - 1]!.char !== " " ? [`${chars[index - 1]!.char}|${item.char}@${index}`] : []);
    });
    const sweep = async (phase: string, label?: string) => {
      // The widths QA found breaking (450–470 before Copy, 530–640 after), and
      // a phone-to-desktop spread around them; a 10 px sweep of 1080 px took
      // over a minute and crashed the renderer on a loaded host.
      for (const width of [360, 390, 420, 440, 450, 460, 470, 480, 500, 530, 560, 600, 640, 700, 768, 900, 1024, 1280, 1440]) {
        await page.setViewportSize({ width, height: 844 });
        if (label) await dialog.locator(".pair-command-copy").evaluate((button, text) => { button.textContent = text; }, label);
        expect(await splitWords(), `no word split at ${width} px (${phase})`).toEqual([]);
      }
    };
    await sweep("before Copy");
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    // QA F02: the visible label is the accessible name.
    const copy = dialog.getByRole("button", { name: "Copy command", exact: true });
    await copy.click();
    await expect(dialog.locator(".pair-command-status")).toHaveText("Command copied");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(command);
    await expect(dialog.getByRole("button", { name: "Copied", exact: true })).toBeVisible();
    // The label reverts after two seconds; hold it at "Copied" through the
    // sweep, the state in which the block used to reflow.
    await sweep("while Copy reads Copied", "Copied");
    await dialog.locator(".pair-command-copy").evaluate((button) => { button.textContent = "Copy command"; });
    await page.setViewportSize({ width: 1280, height: 720 });
    // QA F03: a second copy is announced again (the status empties, then speaks).
    await expect(dialog.getByRole("button", { name: "Copy command", exact: true })).toBeVisible({ timeout: 5_000 });
    // Two copies in a row: each one sets the status anew, so each is spoken.
    const announcements = await dialog.locator(".pair-command-status").evaluate((status) => new Promise<string[]>((resolve) => {
      const seen: string[] = [];
      new MutationObserver(() => seen.push(status.textContent ?? "")).observe(status, { childList: true, characterData: true, subtree: true });
      const button = status.parentElement!.querySelector("button") as HTMLButtonElement;
      button.click();
      setTimeout(() => button.click(), 400);
      setTimeout(() => resolve(seen), 900);
    }));
    expect(announcements.filter((text) => text === "Command copied"), JSON.stringify(announcements)).toHaveLength(2);
  });

  await journey.stage("Re-pair with a code anyway", async () => {
    // The machine's new pairing is a default code pairing: no session launch.
    hub.refuseProofs("atlas", 0);
    hub.setScopes("atlas", [...SCOPES]);
    await dialog.getByRole("button", { name: "Create pairing code" }).click();
    await expect(dialog.getByRole("heading", { name: "Machine authorized" })).toBeVisible({ timeout: 15_000 });
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).toHaveText("Connected", { timeout: 30_000 });
    // cas-b452 (journey F39): Re-pair was started from the cas-src
    // conversation, so it lands back in it, live, with no click to find it again.
    await expect(header).toHaveText(" · Live", { timeout: 30_000 });
    await expect(page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ })).toHaveAttribute("aria-current", "true");
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
  });

  await journey.stage("Told plainly that starting sessions needs allowing again, even after a reload (cas-093d)", async () => {
    // cas-093d F01: the page reloads in between; New session still knows why.
    await page.reload();
    await chooseConversation(page);
    await expect(header).toHaveText(" · Live", { timeout: 30_000 });
    const notice = page.locator("#attention-panel").getByText("Starting sessions needs allowing again").filter({ visible: true });
    await expect(notice).toBeVisible();
    await expect(page.locator("#attention-panel")).toContainText("Re-pairing Atlas · Linux with a code didn't include starting sessions. Open New session to allow it again.");
    // cas-865c: New session keeps its name; without the permission it opens the grant view.
    const toggle = page.locator('#new-session-toggle[data-launch-grant="true"]');
    await expect(toggle).toHaveText("+ New session");
    await toggle.click();
    const sheet = page.getByRole("dialog", { name: "New session" });
    await expect(sheet.locator(".launch-grant .launch-lead")).toHaveText("Re-pairing Atlas · Linux didn't keep starting sessions. Allow it again from this browser.");
  });

  await journey.stage("Allowing it again settles the notice", async () => {
    const sheet = page.getByRole("dialog", { name: "New session" });
    await sheet.getByRole("button", { name: "Allow starting sessions on Atlas · Linux" }).click();
    await expect(sheet.locator('[data-launch-view="form"]')).toBeVisible();
    await sheet.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect(page.locator("#attention-panel").getByText("Starting sessions needs allowing again").filter({ visible: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "New session", exact: true })).toBeVisible();
    // Allowed: the remembered state is gone, so a reload does not bring the lead back.
    expect(await page.evaluate(() => localStorage.getItem("cas-commander-launch-dropped:v1"))).toBeNull();
  });
});

// cas-b00c (journey F19): when several sends in a row lose their receipts
// across a network switch, the thread says so once, with Review, instead of
// stacking a warning card with its own Retry and × for each. The list row
// says the last one is not confirmed and does not date the row by it.
test("HUB-J12 network switch: three unconfirmed messages read as one notice (cas-b00c)", journeyPart, async ({ page, journey }) => {
  await journey.stage("Three unconfirmed messages read as one notice", async () => {
    const { hub, clock } = await connected(page);
    const log = page.getByRole("log");
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    const when = row.locator(".conversation-when");
    const whenBefore = (await when.count()) ? await when.textContent() : null;
    for (const text of ["Is the gate green?", "Did the Mac tests start?", "Ship it if both are green"]) {
      const next = hub.nextSend();
      await sendNow(page, text);
      expect((await next).text).toBe(text);
    }
    await clock.advance(15_000); // every receipt deadline passes
    const notice = log.locator('.conversation-unconfirmed[role="status"]');
    await expect(notice).toHaveCount(1);
    await expect(notice).toHaveText("3 messages not confirmed · Cassy couldn't confirm delivery to the cas-src supervisor. Review them to retry.");
    await expect(log.getByRole("button", { name: "Retry sending" })).toHaveCount(0);
    await expect(log.getByRole("button", { name: /^Dismiss/ })).toHaveCount(0);
    await expect(row).toContainText("Not confirmed: Ship it if both are green");
    // Unconfirmed sends are not activity the supervisor saw: the row's time is unchanged.
    if (whenBefore === null) await expect(when).toHaveCount(0); else await expect(when).toHaveText(whenBefore);
    const review = log.getByRole("button", { name: "Review 3 messages not confirmed", exact: true });
    await review.click();
    await expect(log.getByRole("button", { name: "Retry sending" })).toHaveCount(3);
    await expect(log.getByRole("button", { name: "Retry sending" }).first()).toBeFocused();
    await expect(log.getByRole("button", { name: "Dismiss this notice", exact: true })).toHaveCount(3);
    await log.getByRole("button", { name: /^Show 3 messages not confirmed as one notice$/ }).click();
    await expect(review).toBeFocused();
    // Nothing was sent twice.
    expect(hub.sends).toHaveLength(3);
    // cas-ca7f (cas-b00c QA F01): Review opens one run. With the first run
    // left open, a delivered message and then a new run of two: the new run is
    // one collapsed notice of its own.
    await review.click();
    await expect(log.getByRole("button", { name: "Retry sending" })).toHaveCount(3);
    const delivered = hub.nextSend();
    await sendNow(page, "Status, please");
    expect((await delivered).text).toBe("Status, please");
    hub.deliverLatest(PELICAN);
    for (const text of ["Gate still red?", "Hold the train"]) {
      const next = hub.nextSend();
      await sendNow(page, text);
      expect((await next).text).toBe(text);
    }
    await clock.advance(15_000);
    await expect(log.getByRole("button", { name: "Review 2 messages not confirmed", exact: true })).toHaveAttribute("aria-expanded", "false");
    await expect(log.getByRole("button", { name: "Retry sending" }), "only the first run's three are open").toHaveCount(3);
    // The dismissed-messages chip takes the caution tone for messages only not confirmed.
    for (let index = 0; index < 3; index += 1) await log.getByRole("button", { name: "Dismiss this notice", exact: true }).first().click();
    await expect(page.locator(".conversation-unsent")).toHaveAttribute("data-tone", "caution");
    // cas-6a96 (journey F36): the thread's notice said "2 messages not
    // confirmed" over a chip saying "Show 3 messages not confirmed". The chip
    // counts what was dismissed, and now says so; no two counts of the same
    // thing disagree, on screen or to a screen reader.
    const chip = page.getByRole("button", { name: "Show 3 dismissed messages, not confirmed", exact: true });
    await expect(chip).toBeVisible();
    await expect(chip).toContainText("3 dismissed");
    await expect(log.locator('.conversation-unconfirmed[role="status"] b')).toHaveText("2 messages not confirmed");
    const counts = await page.evaluate(() => {
      const said = [document.body.innerText, ...[...document.querySelectorAll("[aria-label]")].map((node) => node.getAttribute("aria-label") ?? "")].join("\n");
      return [...said.matchAll(/(\d+) (?:messages?|dismissed)[^\n]*?not (?:sent or not )?confirmed/g)].map((match) => `${match[0].includes("dismissed") ? "dismissed" : "thread"}:${match[1]}`);
    });
    expect([...new Set(counts.filter((said) => said.startsWith("thread:")))], "every count of the thread's not-confirmed messages agrees").toEqual(["thread:2"]);
    expect([...new Set(counts.filter((said) => said.startsWith("dismissed:")))], "every count of the dismissed ones agrees").toEqual(["dismissed:3"]);
  });
});

// cas-8f19: what is stored is read with the bounds it was written with. A
// corrupted or foreign-written "waiting" message far over the store's size
// bound is dropped on read: it is never shown as waiting and never sent on
// the operator's behalf when the session is live.
test("HUB-J12 network switch: an oversized stored waiting message is not sent after a reload (cas-8f19)", journeyPart, async ({ page, journey }) => {
  await journey.stage("An oversized stored waiting message is dropped on reload and never sent", async () => {
    const { hub, clock, header, held } = await connected(page);
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    // Stamped from the journey's protocol clock, never the page's ambient Date.
    const now = clock.now();
    await page.evaluate(([key, target, now]) => {
      localStorage.setItem("cas-commander-conversation:sends:v1", JSON.stringify({ [key]: { value: [{ id: "planted", target, text: "x".repeat(2_000_000), state: "held", at: now, heldAt: now }], updatedAt: now } }));
    }, [`atlas:${PELICAN}`, PELICAN, now] as const);
    await page.reload();
    await chooseConversation(page);
    await expect(header).toHaveText(" · Live");
    await expect(held).toHaveCount(0);
    await clock.advance(15_000); // a reattach and a receipt window later
    expect(hub.sends, "nothing was sent on the operator's behalf").toHaveLength(0);
    expect(errors).toEqual([]);
  });
});

// cas-fc2c: a phone that reloads the tab while the session cannot attach yet
// still shows the conversation's kept messages straight away: the waiting
// message is on screen, saying it waits, before the session is back, and it
// goes out once when the session attaches.
test("HUB-J12 network switch: kept messages show before the session attaches after a reload (cas-fc2c)", journeyPart, async ({ page, journey }) => {
  await journey.stage("Kept messages show before attach, then send once without moving the reader", async () => {
    const { hub, clock, held } = await connected(page);
    hub.upstreamLost(PELICAN);
    await sendNow(page, "Kept while the session is away");
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    // The reloaded page's attach is held: the session cannot attach yet.
    const releaseAttach = hub.holdAttach(PELICAN);
    await page.reload();
    await chooseConversation(page);
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    await expect(page.getByRole("log").locator(".bub").filter({ hasText: "Kept while the session is away" })).not.toContainText(/Sending…|Delivered/);
    expect(sentTimes(hub, "Kept while the session is away")).toBe(0);
    // A keyboard reader is in the thread when the session comes back.
    await page.locator(".conversation-reading.thread").focus();
    // The session comes back: the thread is the same one, and the message goes once.
    hub.upstreamBack(PELICAN);
    const next = hub.nextSend();
    releaseAttach();
    await clock.advance(1_000);
    expect((await next).text).toBe("Kept while the session is away");
    await expect(held).toHaveCount(0);
    // The reader stays in the thread that took over, not on the page.
    await expect(page.locator(".conversation-reading.thread")).toBeFocused();
    await expect(page.getByRole("log")).toHaveCount(1);
    await expect(page.getByRole("log").locator(".bub").filter({ hasText: "Kept while the session is away" })).toHaveCount(1);
    await clock.advance(15_000);
    expect(sentTimes(hub, "Kept while the session is away")).toBe(1);
  });
});


test("HUB-J12 explain a machine connection that cannot retry (cas-99d7)", journeyPart, async ({ page, journey }) => {
  await journey.stage("The stopped connection explains what to do", async () => {
    const { hub, clock, header } = await connected(page);
    // Exercise the real fatal transport branch, after a successful connection.
    // A missing required browser API cannot be fixed by another network retry.
    await page.evaluate(() => { Object.defineProperty(AbortSignal, "timeout", { value: undefined, configurable: true }); });
    await hub.down("atlas", { sockets: "close" });
    await clock.advance(2_000);
    const banner = page.locator(".terminal-disconnected-banner .banner-text");
    const recovery = "This browser is missing a feature Cassy Cloud needs. Update to Chrome 103, Edge 103, Firefox 100, or Safari 16 or newer. Then reload this page.";
    // cas-97d58 F16: the browser, not the machine, is named as the cause.
    await expect(banner).toHaveText(`This browser can't connect to Atlas · Linux. ${recovery}`);
    await expect(header).toContainText("Unreachable");
    await expect(page.locator(".status-stale").filter({ visible: true })).toHaveText(/^Not live — This browser is missing a feature Cassy Cloud needs\./);
    await expect(page.locator("#attention-panel").getByText("Lost connection to Atlas · Linux", { exact: true })).toHaveCount(1);
    // No automatic reconnect is claimed on any visible surface: the list
    // footer beside the conversation names the machine as the row and the
    // header do (cas-f698).
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).toHaveText("Unreachable");
    await expect(page.getByText(/reconnecting|return when it reconnects/i).filter({ visible: true })).toHaveCount(0);
    // The conversation's actions say the same step (cas-0546).
    for (const name of ["Interrupt the cas-src supervisor", "Raw output"]) {
      const action = page.getByRole("button", { name, exact: true });
      await expect(action).toHaveAttribute("aria-disabled", "true");
      await expect(action).toHaveAccessibleDescription(`${recovery} Interrupt and raw output wait until then.`);
    }
    await expect(banner).toMatchAriaSnapshot(`- text: This browser can't connect to Atlas · Linux. ${recovery}`);
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    await page.locator("#palette-paired-machines").click();
    // cas-97d58 F16: the browser is the cause, not an unreachable machine.
    await expect(page.locator("#paired-machines-list .paired-machine-state")).toHaveText(`This browser can't connect. ${recovery}`);
  });
});


test("HUB-J12 peer clean exit and retained replay preserve healthy legacy attaches (cas-49cc)", journeyPart, async ({ page, journey }) => {
  await journey.stage("Two healthy machines survive another session's clean exit and its replay", async () => {
    const peer = "removed-peer";
    const atlas = { ...ATLAS, sessions: [...ATLAS.sessions, { ...ATLAS.sessions[0]!, name: peer, supervisor: peer, project_dir: "/projects/peer" }] };
    const { hub, clock, header } = await connected(page, false, [atlas, STUDIO]);
    const nav = page.getByRole("navigation", { name: "Choose a supervisor" });
    await nav.getByRole("button", { name: /peer/ }).click();
    await expect(header).toHaveText(" · Live");
    await nav.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(header).toHaveText(" · Live");
    await chooseConversation(page);
    await expect(header).toHaveText(" · Live");
    const before = [hub.legacySocketOpens.get(PELICAN), hub.legacySocketOpens.get(OTTER)];
    atlas.sessions = [...ATLAS.sessions];
    hub.hold(peer);
    hub.drop(peer);
    const prior = Array.from({ length: 1021 }, (_, index) => ({ kind: "pane_added", sequence: 4180 + index, session: peer }));
    const tail = [
      { kind: "pane_exited", sequence: 5201, session: peer, pane_id: "worker" },
      { kind: "daemon_disconnected", sequence: 5202, session: peer, diagnostic: { cause: { kind: "clean_exit", code: 0 }, next_action: "Inspect the factory daemon log and session metadata; do not infer a cause from a closed socket alone." } },
      { kind: "session_removed", sequence: 5203, session: peer },
    ];
    await page.evaluate(({ prior, tail }) => {
      const wire = window as unknown as { __journeyMachineEvent: (host: string, data: string) => number };
      const emit = (event: unknown) => wire.__journeyMachineEvent("atlas.test", JSON.stringify(event));
      const metadata = { kind: "stream_metadata", epoch: "peer-reset", oldest_sequence: 4180, latest_sequence: 5203 };
      // Initial retained window, its live removal tail, then a replay of the same window.
      for (const event of [metadata, ...prior, { kind: "replay_complete" }, ...tail,
        metadata, ...prior, ...tail, { kind: "replay_complete" }]) emit(event);
    }, { prior, tail });
    await hub.waitFor(() => hub.catalogFetchCount("atlas") > 1);
    for (let step = 0; step < 12; step++) {
      await clock.advance(1_000);
      await expect(header).toHaveText(" · Live");
      expect([hub.legacySocketOpens.get(PELICAN), hub.legacySocketOpens.get(OTTER)]).toEqual(before);
    }
    // A route hint and repeated event-only loss must not condemn either
    // recently speaking terminal. Empty output preserves the visible frame.
    hub.send(PELICAN, { Output: { pane_id: "supervisor", data: [] } });
    hub.send(OTTER, { Output: { pane_id: "supervisor", data: [] } });
    await clock.advance(1);
    await page.evaluate(() => window.dispatchEvent(new Event("online")));
    await clock.advance(1_000);
    expect([hub.legacySocketOpens.get(PELICAN), hub.legacySocketOpens.get(OTTER)]).toEqual(before);
    for (let second = 0; second < 30; second++) {
      hub.send(PELICAN, { Output: { pane_id: "supervisor", data: [] } });
      hub.send(OTTER, { Output: { pane_id: "supervisor", data: [] } });
      await page.evaluate(() => {
        const wire = window as unknown as { __journeyResetEvents: (host: string) => void };
        wire.__journeyResetEvents("atlas.test");
        wire.__journeyResetEvents("studio.test");
      });
      await clock.advance(1_000);
      await expect(header).toHaveText(" · Live");
      expect([hub.legacySocketOpens.get(PELICAN), hub.legacySocketOpens.get(OTTER)]).toEqual(before);
    }
    const streams = await page.evaluate(() => (window as unknown as {
      __journeyEventStreamOpens: Record<string, number>;
    }).__journeyEventStreamOpens);
    expect(streams["atlas.test"]).toBeLessThanOrEqual(6);
    expect(streams["studio.test"]).toBeLessThanOrEqual(6);
    await nav.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(header).toHaveText(" · Live");
  });
});
