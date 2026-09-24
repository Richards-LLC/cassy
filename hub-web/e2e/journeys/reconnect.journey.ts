import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN } from "./world";

test("HUB-J11 the connection drops mid-conversation and recovers", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });

  const header = page.locator("#conversation-connection");
  const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
  const footer = page.locator("#hub-footer-badges");
  const banner = page.locator(".terminal-disconnected-banner");

  await journey.stage("The network drops", async () => {
    await expect(header).toHaveText(" · Live");
    // The outage lasts until released, so a send can be tried while it is down.
    hub.hold(PELICAN);
    hub.drop(PELICAN);
    // One connection state: in the same frame, the banner, the header, the row
    // and the footer all say so. The double retries within about a second, so
    // the surfaces are read together rather than one expect at a time.
    const together = await page.waitForFunction(() => {
      const text = (selector: string) => document.querySelector<HTMLElement>(selector)?.innerText ?? "";
      const seen = {
        banner: text(".terminal-disconnected-banner"),
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
    // Two machines, one of them down: the footer counts it and its dot is not all-clear (cas-b789).
    expect(seen.footer).toContain("1 connected");
    await expect(footer.locator(".pairing-dot")).toHaveClass("pairing-dot partial");
    // A send during the outage is refused for the connection, and says so.
    await composer.fill("Are you there?");
    await page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true }).click();
    await expect(page.locator("#message-status")).toHaveText("The hub connection is reconnecting, so this message was not delivered. Try again once the session is live.");
    hub.release(PELICAN);
  });

  await journey.stage("It reconnects on its own", async () => {
    await expect.poll(() => hub.hasSocket(PELICAN), { timeout: 30_000 }).toBe(true);
    await expect(banner).toBeHidden({ timeout: 15_000 });
    await expect(header).toHaveText(" · Live");
    await expect(row).toContainText("Live");
    await expect(footer).toContainText("Connected");
    await expect(footer.locator(".pairing-dot")).toHaveClass("pairing-dot connected");
    // The reconnecting refusal cleared with the reconnect; the draft is kept to send again (cas-b789).
    await expect(page.locator("#message-status")).toBeHidden();
    await expect(composer).toHaveValue("Are you there?");
    // The transport alarm resolved itself with the reconnect.
    await expect(page.getByText("Terminal transport problem")).toHaveCount(0);
  });

  await journey.stage("Sending works again", async () => {
    await composer.fill("Are we back?");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true }).click();
    expect((await sent).text).toBe("Are we back?");
    hub.answerLatest(PELICAN, "Back. Nothing was lost.");
    await expect(page.getByRole("log").getByText("Back. Nothing was lost.")).toBeVisible();
    await expect(page.getByText("Terminal transport problem")).toHaveCount(0);
  });

  await journey.stage("On a phone, the banner stays readable through an outage", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}` })).toBeVisible();
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
    await expect(banner).toHaveText("Lost connection to Atlas · Linux. Reconnecting…");
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
    await expect(banner).toBeHidden({ timeout: 15_000 });
    await page.emulateMedia({ colorScheme: null });
  });
});
