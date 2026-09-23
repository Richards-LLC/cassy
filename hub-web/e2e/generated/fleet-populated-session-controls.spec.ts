// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

import { test, expect } from "@playwright/test";

test.describe("Populated fleet", () => {
  test("Fleet ledger exposes distinct open controls for all three sessions", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=fleet-populated.
    await page.goto("/?fixture=fleet-populated");
    await expect(page.getByRole("heading", { name: "Session ledger" })).toBeVisible();

    // 2. Locate each ledger button by accessible name: Open bright-otter on Atlas laptop, Open calm-heron on Atlas laptop, and Open quiet-marten on Forge desktop.
    const bright = page.getByRole("button", { name: "Open bright-otter on Atlas laptop", exact: true });
    const calm = page.getByRole("button", { name: "Open calm-heron on Atlas laptop", exact: true });
    const quiet = page.getByRole("button", { name: "Open quiet-marten on Forge desktop", exact: true });

    for (const button of [bright, calm, quiet]) {
      await expect(button).toHaveCount(1);
      await expect(button).toBeEnabled();
    }

    await expect(bright).toHaveAttribute("data-fleet-machine", "atlas");
    await expect(bright.getByText("editing", { exact: true })).toBeVisible();
    await expect(bright).toContainText("3 workers");

    await expect(calm).toHaveAttribute("data-fleet-machine", "atlas");
    await expect(calm.getByText("testing", { exact: true })).toBeVisible();
    await expect(calm).toContainText("1 worker");

    await expect(quiet).toHaveAttribute("data-fleet-machine", "forge");
    await expect(quiet.getByText("blocked", { exact: true })).toBeVisible();
    await expect(quiet).toContainText("6 workers");

    // 3. Activate the Open quiet-marten on Forge desktop button.
    await quiet.click();
    await expect(page.getByRole("heading", { name: "Session ledger" })).toBeVisible();
    await expect(page.locator("#app")).not.toBeEmpty();
    await expect(page).toHaveURL(/\?fixture=fleet-populated$/);
  });
});
