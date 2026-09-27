import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN } from "./world";

// cas-0978: the phone moves between networks. The double speaks the real hub's
// machine protocol (one socket, health ping/pong) so a half-open socket, the
// failure a network switch actually leaves, can be reproduced.
test("HUB-J12 switch networks without losing the conversation", async ({ page, journey }) => {
  // Five transitions, one of them a 25 s outage with four missed heartbeats.
  test.setTimeout(210_000);
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"], multiplex: true });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const send = page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true });
  const header = page.locator("#conversation-connection");
  const held = page.getByRole("log").locator(".conversation-held");
  const sentTimes = (text: string) => hub.sends.filter((m) => m.text === text).length;
  const sendNow = async (text: string) => { await composer.fill(text); await send.click(); };
  // Waits on the double's own counters without recording each miss as a
  // failed expectation; the caller asserts the final value once.
  const until = async (value: () => number, done: (n: number) => boolean, timeout: number): Promise<number> => {
    const end = Date.now() + timeout;
    while (!done(value()) && Date.now() < end) await page.waitForTimeout(100);
    return value();
  };

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(header).toHaveText(" · Live", { timeout: 15_000 });
    expect(hub.machineSocketOpens.get("atlas")).toBe(1);
  });

  await journey.stage("The route changes under the page", async () => {
    // Local network to Tailscale: the same address, new connections. The
    // sockets reset and are replaced within about a second.
    await hub.down("atlas", { sockets: "close" });
    await hub.up("atlas");
    expect(await until(() => hub.machineSocketOpens.get("atlas") ?? 0, (n) => n > 1, 10_000)).toBeGreaterThan(1);
    await expect(header).toHaveText(" · Live");
    await sendNow("After the route change");
    expect(await until(() => sentTimes("After the route change"), (n) => n >= 1, 5_000)).toBe(1);
  });

  await journey.stage("Tailscale goes off, then on again", async () => {
    // No browser event says so; the open socket goes quiet (half-open).
    await hub.down("atlas");
    await expect(header).toHaveText(" · Reconnecting", { timeout: 30_000 });
    // A message written now waits in the thread instead of vanishing into
    // the dead socket, and says so.
    await sendNow("While Tailscale is off");
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    await expect(page.locator("#message-status")).toHaveText("Not connected to Atlas · Linux right now. Your message will go out by itself when it's back.");
    await page.waitForTimeout(5_000);
    expect(sentTimes("While Tailscale is off")).toBe(0);
    const restored = Date.now();
    await hub.up("atlas");
    // Back without a reload, within the 10 s retry ceiling plus a connect.
    await expect(header).toHaveText(" · Live", { timeout: 15_000 });
    expect(Date.now() - restored, "recovered within 15 s of the network returning").toBeLessThan(15_000);
    // The held message went out once, on a fresh socket.
    expect(await until(() => sentTimes("While Tailscale is off"), (n) => n >= 1, 5_000)).toBe(1);
    await expect(held).toHaveCount(0);
    await expect(page.locator("#message-status")).toBeHidden();
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
  });

  await journey.stage("Wi-Fi hands over to cellular", async () => {
    await page.context().setOffline(true);
    await hub.down("atlas", { sockets: "close" });
    // The browser says it went offline: the page says so at once, not after
    // four missed heartbeats.
    await expect(header).toHaveText(" · Reconnecting", { timeout: 3_000 });
    await sendNow("During the handover");
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    await page.waitForTimeout(3_000);
    const restored = Date.now();
    await hub.up("atlas");
    await page.context().setOffline(false);
    // Back online retries now rather than when the backoff timer says.
    await expect(header).toHaveText(" · Live", { timeout: 5_000 });
    expect(Date.now() - restored, "back within 5 s of coming online").toBeLessThan(5_000);
    expect(await until(() => sentTimes("During the handover"), (n) => n >= 1, 5_000)).toBe(1);
    await page.waitForTimeout(1_000);
    expect(sentTimes("During the handover"), "sent once").toBe(1);
  });

  await journey.stage("The page wakes on a half-open socket", async () => {
    // Asleep while the network came and went: the machine answers HTTP again,
    // but the socket from before is dead on the wire.
    await hub.down("atlas");
    await hub.up("atlas");
    const opens = hub.machineSocketOpens.get("atlas") ?? 0;
    const setVisibility = (state: "hidden" | "visible") => page.evaluate((value) => {
      Object.defineProperty(document, "visibilityState", { value, configurable: true });
      document.dispatchEvent(new Event("visibilitychange"));
    }, state);
    await setVisibility("hidden");
    await page.waitForTimeout(6_000);
    const woke = Date.now();
    await setVisibility("visible");
    // A message written the moment the page wakes, while the socket is
    // still in doubt, is held rather than sent into it.
    await sendNow("Right after waking");
    // Waking checks the socket (a health ping, 3 s to answer) and replaces
    // it, well before four missed heartbeats (about 20 s) would.
    expect(await until(() => hub.machineSocketOpens.get("atlas") ?? 0, (n) => n > opens, 8_000)).toBeGreaterThan(opens);
    expect(Date.now() - woke, "socket replaced within 8 s of waking").toBeLessThan(8_000);
    await expect(header).toHaveText(" · Live");
    expect(await until(() => sentTimes("Right after waking"), (n) => n >= 1, 8_000)).toBe(1);
    await sendNow("After waking");
    expect(await until(() => sentTimes("After waking"), (n) => n >= 1, 5_000)).toBe(1);
    expect(sentTimes("Right after waking"), "sent once").toBe(1);
  });

  await journey.stage("A proof refused after a switch retries on its own", async () => {
    // cas-d636 (soundwave, 22:58Z): after the phone slept through a network
    // switch its first proofs reached the hub stale and were refused, and
    // every 401 read as a revoked pairing, so Commander went dark. The first
    // is now retried with a fresh proof; one refused twice backs off like a
    // lost network. It recovers by itself and never asks to re-pair.
    await hub.down("atlas", { sockets: "close" });
    hub.refuseProofs("atlas", 3);
    await hub.up("atlas");
    await expect.poll(() => hub.proofRefusalsLeft("atlas"), { timeout: 20_000, message: "the refused proofs were exercised" }).toBe(0);
    await expect(header).toHaveText(" · Live", { timeout: 20_000 });
    expect(hub.refusedProofs.every((refusal) => refusal.reason === "stale_proof")).toBe(true);
    await expect(page.getByText(/re-pair|needs pairing|no longer paired|was revoked/i)).toHaveCount(0);
    await sendNow("After a refused proof");
    expect(await until(() => sentTimes("After a refused proof"), (n) => n >= 1, 5_000)).toBe(1);
  });
});
