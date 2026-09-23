import { test, expect } from "./journey";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

test("HUB-J3 find the conversation that needs me", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });

  await journey.stage("See every machine's supervisors in one list", async () => {
    await journey.open();
    await expect(list.getByRole("button")).toHaveCount(2);
    await expect(list.locator(".conversation-machine")).toHaveText(["Atlas", "Studio Mac"]);
    await expect(list.locator(".project-badge")).toHaveText(["cas-src", "gabber-studio"]);
  });

  await journey.stage("Notice a new reply while away", async () => {
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await page.getByRole("button", { name: "‹ Conversations", exact: true }).click();
    hub.supervisorSays(PELICAN, "The staging deploy finished; nothing needs you yet.", { kind: "status" });
    await expect(list.getByRole("button", { name: /cas-src/ }).getByLabel("1 unread")).toBeVisible();
  });

  await journey.stage("Jump to a supervisor by name", async () => {
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    await page.getByRole("searchbox", { name: "Filter commands" }).fill(OTTER);
    const commands = page.locator("#command-palette .palette-command");
    await expect(commands.visible()).toHaveCount(1);
    await expect(commands.visible().first()).toContainText(`Jump to ${OTTER}`);
    await expect(page.getByRole("button", { name: /Appearance · Dark/ })).toBeHidden();
    await page.getByRole("button", { name: new RegExp(`Jump to ${OTTER}`) }).click();
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(page.locator(".conversation-host")).toContainText("gabber-studio · Studio Mac");
    // A mouse jump lands in the opened conversation's composer too, and
    // landing there must not freeze the shell at its pre-load state: once
    // this first visit's lease loads, the palette offers "Release control".
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
    await expect(page.locator('#command-palette [data-palette-action="control"]')).toContainText("Release control");
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeFocused();
  });

  await journey.stage("Jump to a supervisor from the keyboard", async () => {
    // Mid-draft in the composer: closing the palette hands focus back to it,
    // which must not hold the switch back either.
    await page.getByRole("textbox", { name: "Your message" }).fill("Half a thought");
    await page.keyboard.press("ControlOrMeta+k");
    const filter = page.getByRole("searchbox", { name: "Filter commands" });
    await expect(filter).toBeFocused();
    await filter.fill(PELICAN);
    await filter.press("Enter");
    // Enter in the filter picks the leading "Jump to" row; the palette must
    // close with it rather than keep the modal up over the opened session.
    await expect(page.locator("#command-palette")).toBeHidden();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(page.locator(".conversation-host")).toContainText("cas-src · Atlas");
    // Focus lands in the opened conversation's composer, so the next keystroke
    // is part of the reply; the other conversation's draft stays its own.
    const composer = page.getByRole("textbox", { name: "Your message" });
    await expect(composer).toBeFocused();
    await expect(composer).toHaveValue("");
    await page.keyboard.type("On it");
    await expect(composer).toHaveValue("On it");
    await page.keyboard.press("ControlOrMeta+k");
    await page.getByRole("searchbox", { name: "Filter commands" }).fill(OTTER);
    await page.getByRole("searchbox", { name: "Filter commands" }).press("Enter");
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(composer).toBeFocused();
    await expect(composer).toHaveValue("Half a thought");
  });
});
