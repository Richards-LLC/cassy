import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Conversation question", () => {
  test("Collapsed gate updates expand and collapse without losing the pending ask", async ({ page }) => {
    const showFullUpdate = page.getByRole("button", { name: "Show full update" });
    const showLess = page.getByRole("button", { name: "Show less" });
    const pendingAsk = page.getByRole("region", { name: "Waiting on you: question from patient-pelican-9" });
    const blocker = page.getByRole("group", { name: "Blocker from patient-pelican-9" });
    const condensedProgress = page.getByText("1 more update · gate 11 of 14 targets green");

    // 1. From a fresh page, navigate to /?fixture=conversation-ask.
    await page.goto("/?fixture=conversation-ask");
    await expect(condensedProgress).toBeVisible();
    await expect(showFullUpdate).toBeVisible();
    await expect(pendingAsk).toBeVisible();

    // 2. Click Show full update.
    await showFullUpdate.click();
    await expect(page.getByText("Gate started · 0 of 14 targets")).toBeVisible();
    await expect(page.getByText("gate 11 of 14 targets green")).toBeVisible();
    await expect(showLess).toBeVisible();
    await expect(pendingAsk).toBeVisible();

    // 3. Click Show less.
    await showLess.click();
    await expect(condensedProgress).toBeVisible();
    await expect(showFullUpdate).toBeVisible();
    await expect(pendingAsk).toBeVisible();
    await expect(blocker).toBeVisible();
  });
});
