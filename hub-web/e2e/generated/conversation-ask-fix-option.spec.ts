import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Conversation question", () => {
  test("Fix in-train quick reply answers the pending question", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=conversation-ask.
    await page.goto("/?fixture=conversation-ask");

    const thread = page.getByLabel("Conversation with patient-pelican-9");
    const waitingRegion = page.getByRole("region", {
      name: "Waiting on you: question from patient-pelican-9",
    });
    const question = "Gate run 33512 failed on that one warning.";
    await expect(waitingRegion).toBeVisible();
    await expect(waitingRegion).toContainText(question);
    await expect(waitingRegion.getByRole("button", { name: "Fix in-train" })).toBeVisible();
    await expect(waitingRegion.getByRole("button", { name: "Ship with allowlist" })).toBeVisible();
    await expect(thread.getByRole("group", { name: "Blocker from patient-pelican-9" })).toContainText(
      "The release gate went red. The train is held"
    );
    await expect(thread.getByText("attention.rs:212 · needless_borrow")).toBeVisible();

    // 2. Click Fix in-train once.
    await waitingRegion.getByRole("button", { name: "Fix in-train" }).click();

    const questionInLog = thread.getByRole("group", { name: "Question from patient-pelican-9" });
    await expect(questionInLog.locator('[aria-label="You replied: Fix in-train"]')).toBeVisible();
    await expect(thread.getByRole("log").getByRole("paragraph").filter({ hasText: /^Fix in-train$/ })).toBeVisible();
    await expect(thread.getByRole("log").getByRole("status")).toHaveText("Sending…");
    await expect(waitingRegion).toBeHidden();
    await expect(page.locator('[aria-label="Conversation context"] [data-section="waiting"]')).toBeHidden();
    await expect(page.locator('[aria-label="Conversation context"] .context-waiting li')).toHaveCount(0);
    await expect(questionInLog.locator('[aria-label="You replied: Ship with allowlist"]')).toHaveCount(0);
    await expect(questionInLog.getByText("Waiting on you — answer below")).toHaveCount(0);
  });
});
