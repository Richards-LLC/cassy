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
    // The pinned bar only points at the question; its options live in the Question group in the log.
    const questionInLog = thread.getByRole("log").getByRole("group", { name: "Question from patient-pelican-9" });
    const options = questionInLog.getByRole("button");
    await expect(options).toHaveCount(2);
    await expect(options.nth(0)).toBeEnabled();
    await expect(options.nth(1)).toBeEnabled();
    await expect(thread.getByRole("group", { name: "Blocker from patient-pelican-9" })).toBeVisible();
    await expect(thread.getByText("attention.rs:212 · needless_borrow")).toBeVisible();

    // 2. Click the first option (Fix in-train) once.
    const chosen = (await options.nth(0).textContent())!.trim();
    const other = (await options.nth(1).textContent())!.trim();
    await options.nth(0).click();

    await expect(questionInLog).toHaveAttribute("data-answered", "true");
    await expect(options).toHaveCount(0);
    await expect(questionInLog.locator(".chip.sent")).toHaveCount(1);
    await expect(questionInLog.locator(".chip.sent")).toContainText(chosen);
    await expect(questionInLog.locator(".chip.sent")).not.toContainText(other);
    await expect(thread.getByRole("log").getByRole("paragraph").filter({ hasText: new RegExp(`^${chosen.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`) })).toBeVisible();
    await expect(thread.getByRole("log").locator('.bub[data-state="sending"] .conversation-delivery')).toBeVisible();
    await expect(waitingRegion).toBeHidden();
    await expect(page.locator('[aria-label="Conversation context"] [data-section="waiting"]')).toBeHidden();
    await expect(page.locator('[aria-label="Conversation context"] .context-waiting li')).toHaveCount(0);
  });
});
