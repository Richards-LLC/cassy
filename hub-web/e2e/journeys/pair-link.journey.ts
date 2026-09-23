import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

// A one-time invitation as `cas hub pair` prints it: 43 base64url characters.
const TOKEN = "q3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ";

test("HUB-J2 pair a machine from a cas hub pair link", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS] });
  const dialog = page.locator("#pair-dialog");

  await journey.stage("Open the link the machine printed", async () => {
    await page.goto(`./#pair=${TOKEN}&hub=atlas&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    await expect(dialog.getByText("One-time invitation ready. Confirm the target hub.")).toBeVisible();
    expect(new URL(page.url()).hash, "the secret leaves the address bar at once").toBe("");
  });

  await journey.stage("Confirm the machine and pair", async () => {
    await dialog.getByRole("textbox", { name: /Machine's hub address/ }).fill("https://atlas.test");
    await dialog.getByRole("textbox", { name: /Machine label/ }).fill("Atlas · Linux");
    await dialog.getByRole("textbox", { name: /Operator label/ }).fill("Daniel");
    await expect(dialog.getByRole("checkbox", { name: "message:send" })).toBeChecked();
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(hub.exchanges[0]).toMatchObject({ token: TOKEN, hub_id: "atlas", operator_label: "Daniel" });
  });

  await journey.stage("Reach the supervisor", async () => {
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    await expect(row).toBeVisible({ timeout: 15_000 });
    await row.click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });
});
