import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Attention panel", () => {
  test("Attention event details disclose and hide diagnostic payload", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=attention-12 and locate the Daemon connection lost article.
    await page.goto("/?fixture=attention-12");
    const article = page.getByRole("article").filter({ hasText: "Daemon connection lost" });
    const details = article.locator("details");
    const summary = details.locator("summary");
    const payload = details.locator("pre");
    const copy = details.getByRole("button", { name: "Copy" });
    const eventCount = page.getByText("12 events need attention");
    await expect(article).toBeVisible();
    await expect(details).not.toHaveAttribute("open");
    await expect(payload).toBeHidden();
    await expect(copy).toBeHidden();
    await expect(eventCount).toBeVisible();

    // 2. Click the article's Details disclosure.
    await summary.click();
    await expect(details).toHaveAttribute("open", "");
    await expect(payload).toBeVisible();
    await expect(payload).toContainText('"fixture": "attention-12"');
    await expect(payload).toContainText('"event": 1');
    await expect(copy).toBeVisible();
    await expect(article.getByText("Daemon connection lost")).toBeVisible();
    await expect(article.getByRole("button", { name: "Retry" })).toBeVisible();
    await expect(eventCount).toBeVisible();

    // 3. Click Details again.
    await summary.click();
    await expect(details).not.toHaveAttribute("open");
    await expect(payload).toBeHidden();
    await expect(copy).toBeHidden();
    await expect(eventCount).toBeVisible();
  });
});
