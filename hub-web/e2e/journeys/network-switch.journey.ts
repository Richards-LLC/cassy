import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN } from "./world";

// cas-0978: the phone moves between networks. The double speaks the real hub's
// machine protocol (one socket, health ping/pong) so a half-open socket, the
// failure a network switch actually leaves, can be reproduced.
test("HUB-J12 switch networks without losing the conversation", async ({ page, journey }) => {
  // Seven transitions, one of them a 25 s outage with four missed heartbeats,
  // plus the held-send backoff wait and the legacy-socket stage.
  test.setTimeout(330_000);
  // Time flows as usual; the fake clock only lets the daemon-link stage jump
  // past the two-minute hold on a held message (cas-a355).
  await page.clock.install();
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
    // A real machine drop keeps the machine-level wording (cas-d15c).
    await expect(page.locator(".terminal-disconnected-banner")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
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

  await journey.stage("The session's daemon link drops for a moment", async () => {
    // cas-0653: the hub and the machine stay reachable, but the session's
    // daemon link is gone. The hub refuses the send as retryable
    // (upstream_unavailable) and closes the session's stream; the message
    // waits in the thread instead of reading "Not sent", and goes out once
    // when the session is live again.
    // cas-d15c: the machine never dropped, so the banner names the
    // conversation, not the machine, and the footer stays Connected; it used
    // to read "Lost connection to Atlas · Linux".
    const banner = page.locator(".terminal-disconnected-banner");
    const footer = page.locator("#hub-footer-badges .machine-badge-state");
    await expect(footer).toHaveText("Connected");
    await page.evaluate(() => {
      const seen: string[] = [];
      (window as unknown as { __footerSeen: string[] }).__footerSeen = seen;
      const sample = () => { const text = document.querySelector<HTMLElement>("#hub-footer-badges .machine-badge-state")?.innerText.trim(); if (text && seen.at(-1) !== text) seen.push(text); };
      new MutationObserver(sample).observe(document.body, { subtree: true, childList: true, characterData: true });
    });
    hub.upstreamLost(PELICAN);
    // The resubscribe answers after 2 s, so the reattach is on screen.
    hub.delayAttach(PELICAN, 2_000);
    const refusalsBefore = hub.upstreamRefusals.length;
    await sendNow("While the daemon link is down");
    expect(await until(() => hub.upstreamRefusals.length, (n) => n > refusalsBefore, 5_000)).toBeGreaterThan(refusalsBefore);
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    await expect(banner).toHaveText("Reconnecting to cas-src… Atlas · Linux is still connected.", { timeout: 10_000 });
    await expect(banner).toHaveAttribute("data-scope", "session");
    expect(sentTimes("While the daemon link is down")).toBe(0);
    hub.upstreamBack(PELICAN);
    expect(await until(() => sentTimes("While the daemon link is down"), (n) => n >= 1, 15_000)).toBe(1);
    await expect(held).toHaveCount(0);
    await expect(header).toHaveText(" · Live");
    await page.waitForTimeout(1_000);
    expect(sentTimes("While the daemon link is down"), "sent once").toBe(1);
    await expect(banner).toBeHidden({ timeout: 15_000 });
    const footerSeen = await page.evaluate(() => (window as unknown as { __footerSeen: string[] }).__footerSeen);
    expect(footerSeen.filter((text) => text !== "Connected"), "the footer while the session reconnected").toEqual([]);

    // cas-a355: when the link stays down, the page backs off (about 1, 2,
    // then 4 s between attempts) instead of resending once a second.
    hub.upstreamLost(PELICAN);
    const stayedDownFrom = hub.upstreamRefusals.length;
    await sendNow("While the daemon link stays down");
    await expect(held).toHaveText("Waiting for the connection — sends when it's back");
    await page.waitForTimeout(8_000);
    expect(hub.upstreamRefusals.length - stayedDownFrom, "refusals in 8 s: backed off, not one a second").toBeLessThanOrEqual(4);
    // Past the two-minute hold the message says Not sent, with Retry, in
    // words that fit: no "re-pair this device", and the composer no longer
    // promises it will go out by itself.
    await page.clock.fastForward("02:00");
    const log = page.getByRole("log");
    await expect(log.getByText("Not sent", { exact: true })).toBeVisible({ timeout: 10_000 });
    await expect(log.getByText("The session didn't come back while it waited.")).toBeVisible();
    await expect(log.getByText(/re-pair/i)).toHaveCount(0);
    await expect(held).toHaveCount(0);
    await expect(page.locator("#message-status")).not.toContainText("go out by itself");
    expect(sentTimes("While the daemon link stays down")).toBe(0);
    // Retry, once the link is back, sends it once.
    hub.upstreamBack(PELICAN);
    await log.getByRole("button", { name: "Retry" }).last().click();
    expect(await until(() => sentTimes("While the daemon link stays down"), (n) => n >= 1, 15_000)).toBe(1);
    await page.waitForTimeout(1_000);
    expect(sentTimes("While the daemon link stays down"), "sent once").toBe(1);
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
    // `until`, not expect.poll: each poll miss would be recorded as a failed
    // expectation in the evidence trace (cas-2036).
    expect(await until(() => hub.proofRefusalsLeft("atlas"), (n) => n === 0, 20_000), "the refused proofs were exercised").toBe(0);
    await expect(header).toHaveText(" · Live", { timeout: 20_000 });
    expect(hub.refusedProofs.every((refusal) => refusal.reason === "stale_proof")).toBe(true);
    await expect(page.getByText(/re-pair|needs pairing|no longer paired|was revoked/i)).toHaveCount(0);
    await sendNow("After a refused proof");
    expect(await until(() => sentTimes("After a refused proof"), (n) => n >= 1, 5_000)).toBe(1);
  });

  await journey.stage("On a legacy socket, a second message sent before the refusal arrives waits too", async () => {
    // cas-2036 (cas-a355 QA N4): a hub without the machine protocol carries
    // each session on its own socket, and stops reading it once it refuses a
    // send (hub/server.rs `proxy_socket`). A message written before that
    // refusal reached the page is never read, so it is held with the first
    // and each goes out once, in order, when the session is back.
    hub.useLegacySockets();
    const legacyOpens = hub.legacySocketOpens.get(PELICAN) ?? 0;
    await page.reload();
    await expect(header).toHaveText(" · Live", { timeout: 15_000 });
    expect(hub.legacySocketOpens.get(PELICAN) ?? 0, "the session is on a legacy socket").toBeGreaterThan(legacyOpens);
    // The refusal takes 1.5 s to arrive, as a slow network's round trip would.
    hub.upstreamLost(PELICAN, { refusalDelayMs: 1_500 });
    const sendsFrom = hub.sends.length;
    const refusalsFrom = hub.upstreamRefusals.length;
    await sendNow("First, on the legacy socket");
    await sendNow("Second, before the refusal arrived");
    await expect(held).toHaveCount(2, { timeout: 5_000 });
    await expect(held.first()).toHaveText("Waiting for the connection — sends when it's back");
    // Three refusals in a row: the next reattach already waits about 4 s.
    expect(await until(() => hub.upstreamRefusals.length - refusalsFrom, (n) => n >= 3, 20_000)).toBeGreaterThanOrEqual(3);
    await expect(held).toHaveCount(2);
    hub.upstreamBack(PELICAN);
    expect(await until(() => sentTimes("Second, before the refusal arrived"), (n) => n >= 1, 20_000)).toBe(1);
    await page.waitForTimeout(1_000);
    expect(sentTimes("First, on the legacy socket"), "first sent once").toBe(1);
    expect(sentTimes("Second, before the refusal arrived"), "second sent once").toBe(1);
    expect(hub.sends.slice(sendsFrom).map((m) => m.text), "in the order they were written").toEqual(["First, on the legacy socket", "Second, before the refusal arrived"]);
    await expect(held).toHaveCount(0);

    // cas-2036 (cas-a355 QA N3): the session stayed live with no receipt
    // (the double sends none). Past the settle window (10 s) the next drop
    // retries after about 1 s, not the 8 s the earlier refusals had reached.
    await page.waitForTimeout(11_000);
    hub.upstreamLost(PELICAN);
    const again = hub.upstreamRefusals.length;
    await sendNow("After the session stayed live");
    expect(await until(() => hub.upstreamRefusals.length - again, (n) => n >= 1, 5_000)).toBeGreaterThanOrEqual(1);
    const firstRefusal = Date.now();
    expect(await until(() => hub.upstreamRefusals.length - again, (n) => n >= 2, 3_000), "retried within about a second").toBeGreaterThanOrEqual(2);
    expect(Date.now() - firstRefusal, "the backoff started afresh").toBeLessThan(3_000);
    hub.upstreamBack(PELICAN);
    expect(await until(() => sentTimes("After the session stayed live"), (n) => n >= 1, 15_000)).toBe(1);
    await page.waitForTimeout(1_000);
    expect(sentTimes("After the session stayed live"), "sent once").toBe(1);
  });
});
