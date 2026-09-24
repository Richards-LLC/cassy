import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

test("HUB-J9 on a phone: from the list to a reply and back", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });

  await journey.stage("Open the list on a phone", async () => {
    await journey.open();
    await expect(list.getByRole("button")).toHaveCount(2);
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
});
