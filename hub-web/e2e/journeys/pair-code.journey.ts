import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

test("HUB-J1 first open and pair a machine with a code", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], relay: { machine: "atlas", claimAfter: 2, authorizeAfter: 4 } });
  const dialog = page.locator("#pair-dialog");

  await journey.stage("Open Cassy Commander for the first time", async () => {
    await journey.open();
    await expect(page.getByRole("heading", { name: "Stay close to the work." })).toBeVisible();
    await expect(page.getByText("Pair a machine to start your first conversation.")).toBeVisible();
  });

  await journey.stage("Ask for a pairing code", async () => {
    await page.getByRole("button", { name: "Pair a machine" }).filter({ visible: true }).click();
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
    await dialog.getByRole("button", { name: "Create pairing code" }).click();
    await expect(dialog.getByText("cas hub authorize KQ7M-4XTR")).toBeVisible();
  });

  await journey.stage("Approve on the machine", async () => {
    await expect(dialog.getByRole("heading", { name: "Machine authorized" })).toBeVisible({ timeout: 15_000 });
    await expect(dialog.getByText("Atlas · Linux").first()).toBeVisible();
  });

  await journey.stage("Confirm and pair this browser", async () => {
    await dialog.getByRole("textbox", { name: /Operator label/ }).fill("Daniel");
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(hub.exchanges).toHaveLength(1);
    expect(hub.exchanges[0]).toMatchObject({ hub_id: "atlas", operator_label: "Daniel", token: "journey-invitation" });
  });

  await journey.stage("See the machine's supervisor ready to talk to", async () => {
    await expect(page.getByRole("status").filter({ hasText: "Atlas · Linux connected" })).toBeVisible({ timeout: 15_000 });
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    await expect(row).toBeVisible();
    await row.click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });
});
