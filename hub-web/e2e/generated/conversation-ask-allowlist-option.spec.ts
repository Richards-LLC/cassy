// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

import { test, expect } from "@playwright/test";

test.describe("Conversation question", () => {
  test("Ship with allowlist quick reply records the alternate answer", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=conversation-ask and locate the pinned gate-run question.
    await page.goto("/?fixture=conversation-ask");

    const log = page.getByRole("log");
    const pinnedQuestion = page.getByRole("region", {
      name: "Waiting on you: question from patient-pelican-9",
    });
    await expect(pinnedQuestion).toBeVisible();
    await expect(pinnedQuestion).toContainText("Gate run 33512 failed");
    await expect(page.getByRole("button", { name: "Fix in-train" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Ship with allowlist" })).toBeVisible();
    await expect(log.locator('[aria-label^="You replied:"]')).toHaveCount(0);

    // 2. Click Ship with allowlist once.
    await page.getByRole("button", { name: "Ship with allowlist" }).click();

    await expect(log.locator('[aria-label="You replied: Ship with allowlist"]')).toBeVisible();
    await expect(log.locator("p").filter({ hasText: /^Ship with allowlist$/ })).toBeVisible();
    await expect(log.getByRole("status")).toHaveText("Sending…");
    await expect(pinnedQuestion).toBeHidden();
    await expect(log.locator('[aria-label="You replied: Fix in-train"]')).toHaveCount(0);
  });
});
