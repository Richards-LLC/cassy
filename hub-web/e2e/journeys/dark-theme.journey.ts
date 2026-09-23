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
    await page.keyboard.press("ControlOrMeta+k");
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
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(page.getByRole("log").getByText("Status: the Linux lane is green; the Mac lane is still running.")).toBeVisible();
  });
});
