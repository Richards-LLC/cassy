import type { Locator } from "@playwright/test";
import { test, expect, expectWholeFocusRing } from "./journey";
import { ATLAS, PELICAN } from "./world";

/** Every field in the dialog is visible without scrolling it (F4). */
async function everyFieldAboveTheFold(dialog: Locator): Promise<void> {
  const fields = dialog.locator("input:visible");
  for (let index = 0; index < await fields.count(); index += 1) await expect(fields.nth(index)).toBeInViewport({ ratio: 1 });
  expect(await dialog.locator("form, .pair-flow").first().evaluate((scroller: Element) => scroller.scrollTop), "the dialog opens unscrolled").toBe(0);
}

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
    await expect(dialog.getByText("This browser will be able to:")).toBeVisible();
    await expect(dialog.getByText("Technical details")).toBeVisible();
    await everyFieldAboveTheFold(dialog);
    await dialog.getByRole("button", { name: "Create pairing code" }).click();
    await expect(dialog.getByText("cas hub authorize KQ7M-4XTR")).toBeVisible();
  });

  await journey.stage("Approve on the machine", async () => {
    await expect(dialog.getByRole("heading", { name: "Machine authorized" })).toBeVisible({ timeout: 15_000 });
    await expect(dialog.getByText("Atlas · Linux").first()).toBeVisible();
    await expect(dialog.getByText("Check this is your machine.")).toBeVisible();
    // One heading per step: "Machine authorized" is said once (cas-b2e4 F01).
    await expect(dialog.getByText("Machine authorized")).toHaveCount(1);
    await expect(dialog.getByText("Add your name, then press Pair.")).toBeVisible();
    await expectWholeFocusRing(dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }));
    await expect(dialog.getByText(/device credential/)).toHaveCount(0);
    await everyFieldAboveTheFold(dialog);
  });

  await journey.stage("Confirm and pair this browser", async () => {
    await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
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
    // The "connected" toast sits at the top, clear of the message box (journey F12).
    // Measure #toast itself: it keeps its box after it fades, so a slow run
    // that outlasts the 3.2 s display cannot hang on the .visible class.
    const toast = page.locator("#toast");
    await expect(toast).toHaveText("Atlas · Linux connected");
    const [notice, composer] = await Promise.all([toast.boundingBox(), page.locator(".conversation-composer").boundingBox()]);
    expect(notice!.y + notice!.height, "toast above the composer").toBeLessThan(composer!.y);
    // It covers no heading either: at the top right it used to land on the
    // context rail's "Tasks & progress" (3.30.0 journey F8).
    const covered = await page.evaluate(() => {
      const t = document.querySelector<HTMLElement>("#toast")!.getBoundingClientRect();
      return [...document.querySelectorAll<HTMLElement>("h1, h2, h3")].filter((h) => h.getClientRects().length > 0).filter((h) => {
        const r = h.getBoundingClientRect();
        return r.width > 0 && t.left < r.right && t.right > r.left && t.top < r.bottom && t.bottom > r.top;
      }).map((h) => h.textContent?.trim());
    });
    expect(covered, "headings under the toast").toEqual([]);
  });
});
