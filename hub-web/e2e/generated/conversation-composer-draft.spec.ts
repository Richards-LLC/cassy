import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Conversation composer", () => {
  test("Composer preserves and edits the seeded draft", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=conversation-composer.
    await page.goto("/?fixture=conversation-composer");
    await expect(page.getByRole("heading", { name: "cas-src", exact: true })).toBeVisible();
    const message = page.getByRole("textbox", { name: "Your message" });
    await expect(message).toHaveValue("Cut 3.26.0 once the gate is green, then post the release notes.");
    await expect(page.getByRole("button", { name: "Send to patient-pelican-9" })).toBeVisible();
    // No dead attach control until attaching works (cas-17e3).
    const attachment = page.getByRole("button", { name: /Attach a file/ });
    await expect(attachment).toHaveCount(0);
    const conversationLog = page.getByRole("log");
    await expect(conversationLog).toContainText("Rebased and pushed; nothing waiting.");
    const originalLog = await conversationLog.textContent();

    // 2. Replace the textbox contents with “Please verify the gate first.”
    await message.fill("Please verify the gate first.");
    await expect(message).toHaveValue("Please verify the gate first.");
    await expect(message).toBeEditable();
    await expect(conversationLog).toHaveText(originalLog);
    await expect(conversationLog).toContainText("Rebased and pushed; nothing waiting.");

    // 3. Clear the textbox.
    await message.fill("");
    await expect(message).toBeEmpty();
    await expect(conversationLog).toHaveText(originalLog);
    await expect(attachment).toHaveCount(0);
  });
});
