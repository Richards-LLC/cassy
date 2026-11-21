import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { HubDouble, type HistoryPage } from "./hub-double";
import { ProtocolClock } from "./protocol-clock";

test("HUB-J12 opaque authenticated requests recover without re-pairing (cas-b85a)", journeyPart, async ({ page, journey }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const clock = new ProtocolClock(page);
  const history: Record<string, HistoryPage[]> = {};
  const hub = new HubDouble(page, { machines: [ATLAS], paired: ["atlas"], multiplex: true, time: clock, history });
  await page.addInitScript(() => { Math.random = () => 0.5; });
  await hub.install();
  let blocked = false;
  let blockedRequests = 0;
  await page.route("https://atlas.test/v1/**", route => {
    // Chrome exposes a blocked preflight/LNA request as an opaque fetch
    // rejection. The credential-free health check can still succeed.
    if (blocked && new URL(route.request().url()).pathname !== "/v1/health") {
      blockedRequests++;
      return route.abort("failed");
    }
    return route.fallback();
  });
  await page.goto("./");
  await hub.seedPaired();
  await clock.start();
  await page.goto("./");
  await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
  const header = page.locator("#conversation-connection");
  await expect(header).toHaveAttribute("data-phase", "live"); await expect(header).toHaveAttribute("data-degraded", "false");
  const pairingExchanges = hub.exchanges.length;
  await journey.stage("A blocked authenticated route keeps the active pairing and retries", async () => {
    blocked = true;
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    await clock.advance(1_000);
    // cas-ee74: an opaque HTTP failure does not condemn a machine whose
    // independent transport/health still responds. Recovery may already be
    // Live at this tick; the contract is retained pairing, not a mandatory
    // dwell in Reconnecting. Prove the blocked route was actually exercised.
    expect(blockedRequests).toBeGreaterThan(0);
    await expect(header).toHaveAttribute("data-phase", /^(?:live|backoff|resolving|dialing|auth|attaching)$/);
    expect(hub.exchanges).toHaveLength(pairingExchanges);
  });
  await journey.stage("Once the browser allows the route, the conversation comes back by itself", async () => {
    history[PELICAN] = [{ messages: [], has_earlier: false, replies: [{
      notification_id: 3539905, reply_to: null, session: PELICAN, kind: "status", attachments: [],
      message: "Queued while the connection was down.", summary: "", device_id: "journey-device",
      at: new Date(clock.now()).toISOString(),
    }] }];
    blocked = false;
    await clock.advance(10_000);
    await expect(header).toHaveAttribute("data-phase", "live"); await expect(header).toHaveAttribute("data-degraded", "false");
    await expect(page.locator("#hub-footer-badges .machine-badge-state")).not.toHaveAttribute("data-reconnecting", "true");
    await expect(page.getByRole("log").getByText("Queued while the connection was down.", { exact: true })).toBeVisible();
    const accepted = hub.nextSend();
    await page.getByRole("textbox", { name: "Your message" }).fill("After the browser block clears");
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect((await accepted).text).toBe("After the browser block clears");
    hub.answerLatest(PELICAN, "The connection recovered.");
    await expect.poll(async () => {
      await clock.advance(32); // drive the coalesced presentation with the protocol clock
      return page.getByRole("log").getByText("The connection recovered.", { exact: true }).isVisible();
    }).toBe(true);
    expect(hub.sends).toHaveLength(1);
    expect(hub.exchanges).toHaveLength(pairingExchanges);
    expect(errors).toEqual([]);
  });
});
