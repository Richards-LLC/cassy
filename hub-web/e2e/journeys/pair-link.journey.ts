import { test, expect } from "./journey";
import { ATLAS, PELICAN, STUDIO } from "./world";

// A one-time invitation as `cas hub pair` prints it: 43 base64url characters.
const TOKEN = "q3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRaQ";
// A first link, for another machine, that the operator opened earlier in the same tab.
const EARLIER_TOKEN = "Zq3VbXo8Zt1nA4wLr9cYp2KdJ6sHf0uEiMgTxBvNyRa";

test("HUB-J2 pair a machine from a cas hub pair link", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS, STUDIO] });
  const dialog = page.locator("#pair-dialog");

  await journey.stage("Open the link the machine printed", async () => {
    // An earlier link for another machine is already open, the name typed.
    await page.goto(`./#pair=${EARLIER_TOKEN}&hub=studio&hub_url=https%3A%2F%2Fstudio.test&machine=Studio%20Mac&scopes=machine:read,session:read,pane:read`);
    await expect(dialog.getByRole("textbox", { name: /Machine label/ })).toHaveValue("Studio Mac");
    // A fresh link opens on the operator's name with the dialog at its top (QA F02).
    await expect(dialog.getByRole("textbox", { name: /Operator label/ })).toBeFocused();
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeInViewport({ ratio: 1 });
    await expect(dialog.getByRole("textbox", { name: /Operator label/ })).toBeInViewport({ ratio: 1 });
    await dialog.getByRole("textbox", { name: /Operator label/ }).fill("Daniel");
    // The link carries the machine's hub address and name, as `cas hub pair`
    // prints it, and arrives in the tab that is already open (hashchange).
    await page.evaluate((hash) => { location.hash = hash; }, `pair=${TOKEN}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas%20%C2%B7%20Linux&scopes=machine:read,session:read,pane:read,pane:input,message:send,pane:interrupt`);
    await expect(dialog.getByText("One-time invitation ready. Confirm the target hub.")).toBeVisible();
    expect(new URL(page.url()).hash, "the secret leaves the address bar at once").toBe("");
  });

  await journey.stage("Confirm the machine and pair", async () => {
    // Address and machine name arrive filled in and editable; only the operator's name is left.
    await expect(dialog.getByRole("textbox", { name: /Machine's hub address/ })).toHaveValue("https://atlas.test");
    await expect(dialog.getByRole("textbox", { name: /Machine's hub address/ })).toBeEditable();
    await expect(dialog.getByRole("textbox", { name: /Machine label/ })).toHaveValue("Atlas · Linux");
    // The name typed for the earlier link is kept; only the machine changed.
    await expect(dialog.getByRole("textbox", { name: /Operator label/ })).toHaveValue("Daniel");
    // The dialog opens at its top: heading and address label on screen (QA F02).
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeInViewport({ ratio: 1 });
    await expect(dialog.getByText("Machine's hub address")).toBeInViewport({ ratio: 1 });
    // The address guidance waits behind a disclosure; its page-origin shortcut is an ordinary button.
    await expect(dialog.getByText("Where do I find this?")).toBeVisible();
    await expect(dialog.getByRole("button", { name: "Use this page's address" })).toBeHidden();
    await dialog.getByText("Where do I find this?").click();
    await expect(dialog.getByRole("button", { name: "Use this page's address" })).toBeVisible();
    await expect(dialog.getByRole("checkbox", { name: "message:send" })).toBeChecked();
    await dialog.getByRole("button", { name: "Pair", exact: true }).click();
    await expect(dialog).toBeHidden();
    expect(hub.exchanges).toHaveLength(1);
    expect(hub.exchanges[0]).toMatchObject({ token: TOKEN, hub_id: "atlas", operator_label: "Daniel" });
    // The new token goes to the new link's machine, never the earlier one (QA F01).
    expect(hub.exchangeOrigins).toEqual(["https://atlas.test"]);
  });

  await journey.stage("Reach the supervisor", async () => {
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    await expect(row).toBeVisible({ timeout: 15_000 });
    await row.click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
  });
});
