import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

test("HUB-J11 the connection drops mid-conversation and recovers", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
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
    expect(seen.footer).toContain("Reconnecting");
    expect(seen.footer).not.toContain("Connected");
  });

  await journey.stage("It reconnects on its own", async () => {
    await expect.poll(() => hub.hasSocket(PELICAN), { timeout: 30_000 }).toBe(true);
    await expect(banner).toBeHidden({ timeout: 15_000 });
    await expect(header).toHaveText(" · Live");
    await expect(row).toContainText("Live");
    await expect(footer).toContainText("Connected");
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
});
