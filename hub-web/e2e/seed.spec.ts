import { test, expect } from "@playwright/test";

// Seed for the Playwright Test Agents. Fixture pages are selected with
// ?fixture=<name>; see FIXTURE_NAMES in fixtures/main.ts.
test.describe("Test group", () => {
  test("seed", async ({ page }) => {
    await page.goto("/?fixture=conversation-composer");
    await expect(page.locator("#app")).not.toBeEmpty();
  });
});
