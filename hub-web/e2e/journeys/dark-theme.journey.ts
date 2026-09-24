import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

test("HUB-J10 switch to dark and keep reading", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });

  await journey.stage("Open the conversation in the light theme", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    hub.supervisorSays(PELICAN, "Status: the Linux lane is green; the Mac lane is still running.", { kind: "status" });
  });

  await journey.stage("Choose the dark appearance", async () => {
    // Ctrl+K lands in the list search; the appearance lives behind the list's Appearance & commands button.
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    // The row in effect says so (F20): System, with a check and "Current".
    const current = page.locator("#command-palette [data-palette-scheme][aria-current='true']");
    await expect(current).toHaveCount(1);
    await expect(current).toHaveAttribute("data-palette-scheme", "system");
    await expect(current.locator(".palette-check")).toBeVisible();
    await expect(current.locator("small")).toHaveText("Current · follows this device");
    await page.getByRole("button", { name: /^Appearance · Dark/ }).click();
    await expect(page.locator("html")).toHaveAttribute("data-scheme", "dark");
  });

  await journey.stage("Keep reading in dark", async () => {
    await expect(page.getByRole("log").getByText("Status: the Linux lane is green; the Mac lane is still running.")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeVisible();
  });

  await journey.stage("The choice survives a reload", async () => {
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("data-scheme", "dark");
    await expect(page.locator("#command-palette [data-palette-scheme='dark']")).toHaveAttribute("aria-current", "true");
    await expect(page.locator("#command-palette [data-palette-scheme='dark'] small")).toHaveText("Current");
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(page.getByRole("log").getByText("Status: the Linux lane is green; the Mac lane is still running.")).toBeVisible();
  });
});
