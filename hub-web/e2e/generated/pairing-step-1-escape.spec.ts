import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Pairing entry", () => {
  test("Escape dismisses the pairing modal", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=pairing-step-1.
    await page.goto("/?fixture=pairing-step-1");
    const dialog = page.locator("dialog#pair-dialog");
    const fleetOverview = page.getByRole("button", { name: "Fleet overview" });
    const workspace = page.getByText("Pairing workspace", { exact: true });
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute("open");
    await expect(page.locator("dialog#pair-dialog:modal")).toBeVisible();
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
    await expect(workspace).toBeVisible();

    // 2. Press Escape.
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(fleetOverview).toBeVisible();
    await expect(workspace).toBeVisible();
    await fleetOverview.focus();
    await expect(fleetOverview).toBeFocused();
  });
});
