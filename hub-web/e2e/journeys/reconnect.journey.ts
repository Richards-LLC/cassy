import { activate, showConversationList, openConversation } from "./responsive-goals";
import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN } from "./world";

// cas-4ce5: recovery is driven by the app's next scheduled attach, which is at
// most MACHINE_RETRY_CEILING_MS (10 s) away however long the outage lasted.
// Wait for that attach to reach the double (bounded by the retry contract plus
// a loaded runner's handshake), then for the screen to follow it.
const RETRY_CONTRACT_MS = 10_000;
const RECOVERY_TIMEOUT_MS = RETRY_CONTRACT_MS + 20_000;

test("HUB-J11 the connection drops mid-conversation and recovers", async ({ page, journey }) => {
  // Three outages, each bounded by RECOVERY_TIMEOUT_MS on a loaded runner (cas-4ce5).
  test.setTimeout(150_000);
  const originalViewport = page.viewportSize()!;
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await openConversation(page, "cas-src");
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
  });

  const header = page.locator("#conversation-connection");
  const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
  const footer = page.locator("#hub-footer-badges");
  const banner = page.locator(".terminal-disconnected-banner");
  /** The thread as painted, top to bottom: day and session lines by text, message groups by their spoken label. */
  /** The journey's own turns as painted, top to bottom (cas-eb4b). */
  const SAID = ["Are you there?", "Are we back?", "Back. Nothing was lost."];
  const turnOrder = () => page.getByRole("log").evaluate((log, said) => {
    const text = log.textContent ?? "";
    return said.map((line) => ({ line, at: text.indexOf(line) })).filter(({ at }) => at >= 0).sort((a, b) => a.at - b.at).map(({ line }) => line);
  }, SAID);
  const threadOrder = () => page.locator(".msgs > *").evaluateAll((nodes) => nodes.filter((node) => node.matches(".day, .session-divider, [role=group]")).map((node) => node.getAttribute("role") === "group" ? node.getAttribute("aria-label") ?? "" : node.textContent ?? ""));
  let beforeOutage: string[] = [];
  /**
   * Journey F42: what the live regions say, one entry per change of words in
   * a region that speaks (role status/alert/log or aria-live, not "off").
   */
  const listen = () => page.evaluate(() => {
    const w = window as unknown as { __said: string[]; __listening?: boolean };
    w.__said = [];
    if (w.__listening) return;
    w.__listening = true;
    const last = new WeakMap<Element, string>();
    const region = (node: Node): HTMLElement | null => {
      for (let element = node instanceof Element ? node : node.parentElement; element; element = element.parentElement) {
        const live = element.getAttribute("aria-live");
        if (live === "off") return null;
        if (live || ["status", "alert", "log"].includes(element.getAttribute("role") ?? "")) return element as HTMLElement;
      }
      return null;
    };
    new MutationObserver((records) => {
      for (const record of records) {
        const speaker = region(record.target);
        if (!speaker || !speaker.isConnected) continue;
        const words = speaker.innerText.trim();
        if (!words || last.get(speaker) === words) continue;
        last.set(speaker, words);
        w.__said.push(words);
      }
    }).observe(document.body, { subtree: true, childList: true, characterData: true });
  });
  const heard = () => page.evaluate(() => (window as unknown as { __said: string[] }).__said);
  const OUTAGE = /lost connection|reconnecting/i;

  await journey.stage("The network drops", async () => {
    await expect(header).toHaveText(" · Live");
    await listen();
    // The outage lasts until released, so a send can be tried while it is down.
    hub.hold(PELICAN);
    hub.drop(PELICAN);
    // One connection state: in the same frame, the banner, the header, the row
    // and the footer all say so. The double retries within about a second, so
    // the surfaces are read together rather than one expect at a time.
    const together = await page.waitForFunction(() => {
      const text = (selector: string) => document.querySelector<HTMLElement>(selector)?.innerText ?? "";
      const seen = {
        banner: text(".terminal-disconnected-banner .banner-text"),
        header: text("#conversation-connection"),
        row: text('#conversation-list [data-thread-key="atlas:patient-pelican-9"]'),
        footer: text("#hub-footer-badges"),
      };
      return seen.banner && seen.header.includes("Reconnecting") ? seen : false;
    });
    const seen = await together.jsonValue() as Record<string, string>;
    expect(seen.banner).toBe("Lost connection to Atlas · Linux. Reconnecting…");
    expect(seen.header).toContain("Reconnecting");
    expect(seen.row).toContain("Reconnecting");
    // Journey F42: the banner announces the outage, once. The header's
    // "Reconnecting" is on screen but is not a second announcement.
    await page.waitForTimeout(1_500);
    expect((await heard()).filter((words) => OUTAGE.test(words)), "the outage is announced once").toEqual(["Lost connection to Atlas · Linux. Reconnecting…"]);
    await expect(header).toHaveAttribute("aria-live", "off");
    await expect(banner.getByRole("status")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
    await expect(banner.getByRole("button", { name: "Connection details" })).toBeVisible();
    // Two machines, one of them down: the footer names it (cas-0739) and its dot is not all-clear (cas-b789).
    expect(seen.footer).toContain("Reconnecting to Atlas");
    await expect(footer.locator(".pairing-dot")).toHaveClass("pairing-dot partial");
    // Only the terminal dims: the conversation stays readable while it
    // reconnects (cas-3446 measured 2.2-3.3:1 when the whole mount faded).
    const readingOpacity = await page.locator(".conversation-reading").evaluate((element) => {
      let product = 1;
      for (let node: Element | null = element; node; node = node.parentElement) {
        product *= Number(getComputedStyle(node).opacity);
      }
      return product;
    });
    expect(readingOpacity, "the conversation reading view is not faded during an outage").toBe(1);
    // A send during the outage is held, not refused: it waits in the thread
    // and goes out by itself, once, when the session is back (cas-0978).
    await composer.fill("Are you there?");
    await activate(page, page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }));
    await expect(page.locator("#message-status")).toHaveText("Lost connection to Atlas · Linux. Reconnecting… Your message will go out by itself when it's back.");
    await expect(composer).toHaveValue("");
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveText("Waiting for the connection — sends when it's back");
    expect(hub.sends.filter((m) => m.text === "Are you there?")).toHaveLength(0);
    // cas-5a8f: a send held in this browser is not supervisor execution: the
    // thread does not say "working" beside "Waiting for the connection", on
    // screen or to a screen reader.
    await expect(page.getByRole("log").locator(".working")).toHaveCount(0);
    expect(await page.getByRole("log").ariaSnapshot()).not.toContain("status: working");
    // The rail defers to the banner: no second, technical alarm about the same
    // drop, and whatever it does show counts the same in every place (cas-90d4).
    const rail = page.locator("#attention-panel");
    await expect(rail).toBeAttached();
    await expect(rail).not.toContainText(/transport/i);
    await expect(rail.getByRole("button", { name: "Open conversation" })).toHaveCount(0);
    const railCounts = await rail.evaluate((element) => {
      const summary = element.querySelector<HTMLElement>(".attention-panel-summary");
      const stated = summary && !summary.hidden ? Number.parseInt(summary.textContent ?? "0", 10) : 0;
      const grouped = [...element.querySelectorAll(".attention-group-count")].reduce((total, count) => total + Number(count.textContent), 0);
      return { stated, grouped };
    });
    expect(railCounts.grouped, "the rail's group counts add up to its summary").toBe(railCounts.stated);
    hub.release(PELICAN);
  });

  await journey.stage("It reconnects on its own", async () => {
    await expect.poll(() => hub.hasSocket(PELICAN), { timeout: RECOVERY_TIMEOUT_MS }).toBe(true);
    await expect(banner).toBeHidden({ timeout: 15_000 });
    await expect(header).toHaveText(" · Live");
    // The header speaks again for the return to Live.
    await expect(header).toHaveAttribute("aria-live", "polite");
    // The row previews the held message now, so it no longer shows the Live
    // status line; it must not say Reconnecting either.
    await showConversationList(page);
    await expect(row).not.toContainText("Reconnecting");
    await openConversation(page, "cas-src");
    await expect(footer).toContainText("Connected");
    await expect(footer.locator(".pairing-dot")).toHaveClass("pairing-dot connected");
    // The held message went out on its own, exactly once, and the waiting
    // line cleared with the reconnect (cas-0978, cas-b789).
    await expect.poll(() => hub.sends.filter((m) => m.text === "Are you there?").length, { timeout: 10_000 }).toBe(1);
    await expect(page.locator("#message-status")).toBeHidden();
    await expect(page.getByRole("log").locator(".conversation-held")).toHaveCount(0);
    // cas-71f4: while it still says "Sending…", that bubble is the one
    // sending signal; no working line beside it.
    await expect(page.getByRole("log").locator(".working")).toHaveCount(0);
    hub.deliverLatest(PELICAN);
    await expect(page.getByRole("log").getByText("Delivered")).toBeVisible();
    // cas-5a8f: once the held send is out on a live machine and delivered,
    // the supervisor has it and the thread says it is working again.
    await expect(page.getByRole("log").locator(".working")).toHaveCount(1);
    await page.waitForTimeout(1_000);
    expect(hub.sends.filter((m) => m.text === "Are you there?"), "sent once, not again").toHaveLength(1);
    // The transport alarm resolved itself with the reconnect.
    await expect(page.locator("#attention-panel")).not.toContainText(/transport/i);
  });

  await journey.stage("Sending works again", async () => {
    await composer.fill("Are we back?");
    const sent = hub.nextSend();
    await activate(page, page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }));
    expect((await sent).text).toBe("Are we back?");
    hub.answerLatest(PELICAN, "Back. Nothing was lost.");
    await expect(page.getByRole("log").getByText("Back. Nothing was lost.")).toBeVisible();
    await expect(page.locator("#attention-panel")).not.toContainText(/transport/i);
    beforeOutage = await threadOrder();
    // cas-eb4b: a session's own thread has no "session … started" line
    // (cas-55a4), so the order is anchored on lines that exist: the day line
    // heads the thread, and the turns stay in the order they were said.
    expect(beforeOutage.filter((line) => line.startsWith("session ")), "no session line in the session's own thread").toEqual([]);
    const today = beforeOutage.indexOf("Today");
    expect(today, "the day line is painted").toBeGreaterThanOrEqual(0);
    expect(today, "the day line comes before the first message").toBeLessThan(beforeOutage.findIndex((line) => line.startsWith("You, ")));
    expect(await turnOrder(), "turns in the order they were said").toEqual(SAID);
  });

  await journey.stage("On a phone, the banner stays readable through an outage", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor" })).toBeVisible();
    // Watch every frame of the outage: no toast may sit on the reconnect banner (cas-00cc).
    await page.evaluate(() => {
      const w = window as unknown as { __covered: string[] };
      w.__covered = [];
      const tick = () => {
        const banner = document.querySelector<HTMLElement>(".terminal-disconnected-banner");
        const toast = document.querySelector<HTMLElement>("#toast.visible");
        if (banner && toast) {
          const b = banner.getBoundingClientRect(), t = toast.getBoundingClientRect();
          if (t.left < b.right && t.right > b.left && t.top < b.bottom && t.bottom > b.top) w.__covered.push(toast.innerText);
        }
        if (w.__covered !== undefined) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    hub.hold(PELICAN);
    hub.drop(PELICAN);
    await expect(banner.locator(".banner-text")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
    // Turning the phone resizes the terminal, which tries to tell the hub: that
    // send fails while the connection is down, which used to raise a toast.
    await page.setViewportSize({ width: 390, height: 760 });
    await page.setViewportSize({ width: 390, height: 844 });
    // The banner's own text is what sits on top at its centre, in light and in dark.
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.waitForTimeout(1_600);
      const onTop = await banner.evaluate((el) => { const r = el.getBoundingClientRect(); const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); return !!hit && (hit === el || el.contains(hit)); });
      expect(onTop, `banner unobscured (${scheme})`).toBe(true);
    }
    expect(await page.evaluate(() => (window as unknown as { __covered: string[] }).__covered), "a toast covered the banner").toEqual([]);
    hub.release(PELICAN);
    await expect.poll(() => hub.hasSocket(PELICAN), { timeout: RECOVERY_TIMEOUT_MS }).toBe(true);
    await expect(banner).toBeHidden({ timeout: 15_000 });
    await page.emulateMedia({ colorScheme: null });
    // cas-1f13: the reconnect re-hydrates the thread from history, and every
    // turn keeps its place: the day line still heads the thread and the held
    // message stays above the ones sent after it (cas-eb4b).
    await expect.poll(() => hub.hasSocket(PELICAN), { timeout: RECOVERY_TIMEOUT_MS }).toBe(true);
    await expect.poll(threadOrder, { message: "thread order after the reconnect" }).toEqual(beforeOutage);
    await expect.poll(turnOrder, { message: "turns in the order they were said, after the reconnect" }).toEqual(SAID);
  });

  await journey.stage("During an outage, Interrupt and Raw output say why they wait", async () => {
    // cas-edcd / cas-4a93: beside "Lost connection … Reconnecting…" nothing
    // says all clear. cas-1730, cas-0546: the conversation header's Interrupt
    // and Raw output stay in place, say why they wait in the banner's words,
    // to the eye and to a screen reader, and come back with the connection.
    await page.setViewportSize(originalViewport);
    const interrupt = page.getByRole("button", { name: "Interrupt the cas-src supervisor", exact: true });
    const raw = page.getByRole("button", { name: "Raw output", exact: true });
    const rail = () => page.evaluate(() => document.querySelector<HTMLElement>("#attention-panel .attention-empty p")?.innerText.trim() ?? "");
    for (const action of [interrupt, raw]) await expect(action).not.toHaveAttribute("aria-disabled", "true");
    await expect.poll(rail).toBe("All clear");
    await listen();
    hub.hold(PELICAN);
    hub.drop(PELICAN);
    // The sentence is the banner's live words; its Details button sits beside them (cas-2b3a5).
    await expect(banner.locator(".banner-text")).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
    await expect(banner.getByRole("button", { name: "Connection details", exact: true })).toBeVisible();
    await expect(header).toContainText("Reconnecting");
    await expect.poll(rail).toBe("Not all clear. Atlas · Linux is reconnecting.");
    const outage = "Lost connection to Atlas · Linux. Interrupt and raw output return when it reconnects.";
    for (const width of [1280, 390]) {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 720 });
      for (const action of [interrupt, raw]) {
        await expect(action, `on screen at ${width}`).toBeVisible();
        await expect(action).toHaveAttribute("aria-disabled", "true");
        await expect(action).toHaveAccessibleDescription(outage);
      }
    }
    // Journey F42: the reasons are descriptions, not live regions: the
    // outage is still announced once, by the banner.
    await page.waitForTimeout(1_000);
    expect((await heard()).filter((words) => OUTAGE.test(words)), "the outage is announced once").toEqual(["Lost connection to Atlas · Linux. Reconnecting…"]);
    // Playwright will not click an aria-disabled control; a person can. A
    // press says why instead of acting: nothing reaches the hub and no drawer
    // opens over the thread.
    const interruptsBefore = hub.frames.filter((frame) => frame.kind === "InterruptPane").length;
    await interrupt.dispatchEvent("click");
    await expect(page.locator("#toast")).toHaveText(outage);
    await raw.dispatchEvent("click");
    await expect(page.getByRole("dialog", { name: "Raw output" })).toBeHidden();
    expect(hub.frames.filter((frame) => frame.kind === "InterruptPane"), "no interrupt during the outage").toHaveLength(interruptsBefore);
    await expect(page.getByRole("log").getByText("Back. Nothing was lost.")).toBeVisible();
    await page.setViewportSize(originalViewport);
    hub.release(PELICAN);
    await expect.poll(() => hub.hasSocket(PELICAN), { timeout: RECOVERY_TIMEOUT_MS }).toBe(true);
    await expect(banner).toBeHidden({ timeout: 15_000 });
    await expect(header).toHaveText(" · Live");
    await expect.poll(rail, { timeout: 15_000 }).toBe("All clear");
    for (const action of [interrupt, raw]) {
      await expect(action).not.toHaveAttribute("aria-disabled", "true");
      await expect(action).toHaveAccessibleDescription("");
    }
    // cas-97d58 F10: the outage's "return when it reconnects" toast ends with
    // the outage, on screen and in the accessibility tree.
    await expect(page.getByText(outage)).toHaveCount(0);
  });
});
