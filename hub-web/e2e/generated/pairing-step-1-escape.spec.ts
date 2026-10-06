import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Pairing entry", () => {
  test("Escape dismisses the pairing modal", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=pairing-step-1.
    await page.goto("/?fixture=pairing-step-1");
    const dialog = page.locator("dialog#pair-dialog");
    const conversations = page.getByRole("heading", { name: "Conversations", exact: true });
    const pairToggle = page.locator("#pair-toggle");
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute("open");
    await expect(page.locator("dialog#pair-dialog:modal")).toBeVisible();
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
    await expect(conversations).toBeVisible();

    // 2. Press Escape.
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(conversations).toBeVisible();
    await expect(pairToggle).toBeVisible();
    await pairToggle.focus();
    await expect(pairToggle).toBeFocused();
  });
});
