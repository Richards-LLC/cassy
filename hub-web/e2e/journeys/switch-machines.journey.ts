import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

test("HUB-J8 switch between machines without losing my place", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const composer = page.getByRole("textbox", { name: "Your message" });
  const back = page.getByRole("button", { name: "‹ Conversations", exact: true });

  await journey.stage("Start a draft on the Linux machine", async () => {
    await journey.open();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator(".conversation-host")).toContainText("cas-src · Atlas · Linux");
    await composer.fill("Draft: ask about the flaky pairing test");
  });

  await journey.stage("Switch to the Mac and send there", async () => {
    await back.click();
    await list.getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.locator(".conversation-host")).toContainText("gabber-studio · Studio Mac");
    await expect(composer).toHaveValue("");
    await composer.fill("Is the Mac build green?");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: `Send to ${OTTER}`, exact: true }).click();
    expect(await sent).toMatchObject({ machine: "studio", target: OTTER, text: "Is the Mac build green?" });
  });

  await journey.stage("Come back to the draft", async () => {
    await back.click();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.locator(".conversation-host")).toContainText("Atlas · Linux");
    await expect(composer).toHaveValue("Draft: ask about the flaky pairing test");
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });

  await journey.stage("Reopen the session picker after closing it", async () => {
    const picker = page.locator("#session-picker");
    const toggle = page.locator("#session-picker-toggle");
    const palette = page.locator("#command-palette");
    await page.getByRole("button", { name: "Terminal view" }).click();
    await expect(toggle).toBeVisible();
    // Closing the picker does not rebuild the shell, and the next periodic
    // render papers over a stale "open" state within a few seconds. So each
    // check is short: the picker must open, and say so, at once — not when a
    // later render happens to rebuild the shell.
    const soon = { timeout: 1_000 };
    const open = async () => {
      await toggle.click();
      await expect(picker).toBeVisible(soon);
      await expect(toggle).toHaveAttribute("aria-expanded", "true", soon);
    };
    const closed = async () => {
      await expect(picker).toBeHidden(soon);
      await expect(toggle).toHaveAttribute("aria-expanded", "false", soon);
    };
    await open();
    await page.keyboard.press("Escape");
    await closed();
    await open();
    await page.getByRole("button", { name: "Close session picker" }).click();
    await closed();
    await open();
    // A closed picker must not pop back open over the next dialog either.
    await page.keyboard.press("Escape");
    await closed();
    await page.keyboard.press("ControlOrMeta+k");
    await expect(palette).toBeVisible(soon);
    await expect(picker).toBeHidden();
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();
    await expect(picker).toBeHidden();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
  });
});
