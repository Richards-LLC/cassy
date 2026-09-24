import { test, expect } from "./journey";
import type { Machine } from "./hub-double";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

/** A paired machine that is switched off: it never answers this visit (cas-b789). */
const SHED: Machine = { id: "shed", label: "Shed NAS · Linux", sessions: [] };
/** A machine paired from the phone mid-journey (cas-002e). */
const FORGE: Machine = { id: "forge", label: "Forge · Linux", sessions: [{ name: "steady-wren-3", supervisor: "steady-wren-3", project_dir: "/projects/forge-tools", workers: ["quick-finch-8"], liveness: "live" }] };

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

test("HUB-J9 on a phone: from the list to a reply and back", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO, SHED, FORGE], paired: ["atlas", "studio", "shed"], relay: { machine: "forge", claimAfter: 2, authorizeAfter: 4 } });
  await page.route("https://shed.test/**", (route) => route.abort("connectionrefused"));
  await page.routeWebSocket(/shed\.test/, (ws) => { void ws.close({ code: 1006 }); });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the list on a phone", async () => {
    // Record every word the list and the footer badge show during the cold
    // load: they must not claim "Not paired", "No live supervisors" or
    // "Reconnecting" before the rows arrive (journey F14).
    await page.addInitScript(() => {
      const seen = new Set<string>();
      (window as unknown as { __coldLoadText: Set<string> }).__coldLoadText = seen;
      const record = () => {
        for (const selector of ["#hub-footer-badges", "#conversation-empty:not([hidden])"]) {
          const text = document.querySelector(selector)?.textContent?.trim();
          if (text) seen.add(text);
        }
      };
      new MutationObserver(record).observe(document, { subtree: true, childList: true, characterData: true, attributes: true });
    });
    await journey.open();
    await expect(list.getByRole("button")).toHaveCount(2);
    const coldLoad = await page.evaluate(() => [...(window as unknown as { __coldLoadText: Set<string> }).__coldLoadText]);
    expect(coldLoad.join(" | "), "cold-load list and footer text").not.toMatch(/Not paired|No live supervisors|Reconnecting/);
    expect(coldLoad.some((text) => text.includes("Loading")), "the cold load shows it is loading").toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scrolling").toBe(true);
    // The compose button says what it does and keeps clear of the status footer (journey F15).
    const compose = page.getByRole("button", { name: "Write to a supervisor" });
    await expect(compose).toHaveText("Write to a supervisor");
    const [button, footer] = await Promise.all([compose.boundingBox(), page.locator(".conversation-sidebar > footer").boundingBox()]);
    expect(button!.y + button!.height, "compose button above the status footer").toBeLessThanOrEqual(footer!.y);
  });

  await journey.stage("Tap a conversation", async () => {
    await list.getByRole("button", { name: /cas-src/ }).tap();
    await expect(list).toBeHidden();
    await expect(page.getByRole("button", { name: "‹ Conversations", exact: true })).toBeVisible();
  });

  await journey.stage("Reply with the phone keyboard", async () => {
    await composer.tap();
    await composer.fill("On my phone — go ahead with the cut.");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true }).tap();
    expect((await sent).text).toBe("On my phone — go ahead with the cut.");
    hub.answerLatest(PELICAN, "Cutting now.");
    await expect(page.getByRole("log").getByText("Cutting now.")).toBeVisible();
  });

  await journey.stage("Go back to the list", async () => {
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).tap();
    await expect(list).toBeVisible();
    await expect(list.getByRole("button", { name: /cas-src/ })).toContainText("Cutting now.");
  });

  await journey.stage("Jump from the palette with a tap", async () => {
    await page.getByRole("button", { name: "Appearance & commands" }).tap();
    // On a phone every command name reads in full: the description drops under
    // it instead of squeezing it (cas-cfcb, cas-5478).
    const titles = page.locator("#command-palette .palette-command:not([hidden]) > span");
    await expect(titles.first()).toBeVisible();
    // Advanced too: its long plain names are the ones that used to clip first.
    await page.locator("#command-palette .palette-advanced > summary").tap();
    await expect(page.getByRole("button", { name: /Open the terminal view/ })).toBeVisible();
    const clipped = await titles.evaluateAll((spans) => spans.filter((span) => span.getClientRects().length > 0 && span.scrollWidth > span.clientWidth + 1).map((span) => span.textContent));
    expect(clipped, "command names cut off at 390 px").toEqual([]);
    await expect(page.locator("#command-palette [data-palette-machine] small").filter({ hasText: "gabber-studio" })).toBeVisible();
    await page.getByRole("button", { name: new RegExp(`Jump to ${OTTER}`) }).tap();
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    // Like a tap on a list row: land to read, with no soft keyboard raised
    // over the conversation just opened.
    await expect(composer).not.toBeFocused();
  });

  await journey.stage("See the switched-off machine named plainly", async () => {
    // Never live and failing: "Can't reach · retrying" in the dialog, not
    // "Connecting…" forever; the footer counts it and its dot is not all-clear.
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).tap();
    const footer = page.locator("#paired-machines-toggle");
    await expect(footer).toContainText("2 connected");
    await expect(footer.locator(".pairing-dot")).toHaveClass("pairing-dot partial");
    await footer.tap();
    const dialog = page.locator("#paired-machines-dialog");
    await expect(dialog.getByText("Shed NAS · Linux")).toBeVisible();
    await expect(dialog).toContainText("Can't reach · retrying");
    await expect(dialog).not.toContainText("Connecting");
  });

  await journey.stage("Pair another machine and read its header at once", async () => {
    // The paired-machines dialog from the stage before is still open.
    await page.keyboard.press("Escape");
    await expect(page.locator("#paired-machines-dialog")).toBeHidden();
    const dialog = page.locator("#pair-dialog");
    await page.getByRole("button", { name: "Pair a machine" }).filter({ visible: true }).first().tap();
    await dialog.getByRole("button", { name: "Create pairing code" }).tap();
    await expect(dialog.getByRole("heading", { name: "Machine authorized" })).toBeVisible({ timeout: 15_000 });
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
    await dialog.getByRole("button", { name: "Pair", exact: true }).tap();
    await expect(dialog).toBeHidden();
    const toast = page.locator("#toast");
    await expect(toast).toHaveText("Forge · Linux connected", { timeout: 15_000 });
    await list.getByRole("button", { name: /forge-tools/ }).tap();
    await expect(page.locator(".conversation-identity h1")).toHaveText("forge-tools");
    // The "connected" toast sits below the thread header, never over the
    // back link, project and host (cas-002e).
    const [notice, heading] = await Promise.all([toast.boundingBox(), page.locator(".conversation-heading").boundingBox()]);
    expect(notice!.y, "toast below the thread header").toBeGreaterThanOrEqual(heading!.y + heading!.height);
  });
});
