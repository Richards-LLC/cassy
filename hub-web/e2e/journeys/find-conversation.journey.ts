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
    await expect(page.getByRole("button", { name: `Send to ${OTTER}`, exact: true })).toBeVisible();
    await expect(page.locator(".conversation-host")).toContainText("gabber-studio · Studio Mac");
  });
});
