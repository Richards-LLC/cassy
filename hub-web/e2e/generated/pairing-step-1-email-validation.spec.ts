import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Pairing entry", () => {
  test("Pairing entry exposes exact scopes and validates optional email", async ({ page }) => {
    const email = page.getByRole("textbox", { name: "Email me the code too (optional)" });
    const createCode = page.getByRole("button", { name: "Create pairing code" });
    const dialog = page.getByRole("dialog");

    // 1. From a fresh page, navigate to /?fixture=pairing-step-1.
    await page.goto("/?fixture=pairing-step-1");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
    await expect(dialog.getByText("This browser will be able to:")).toBeVisible();
    // The exact origin and scopes are one tap away under Technical details.
    await expect(dialog.getByText(new URL(page.url()).origin, { exact: true })).toBeHidden();
    await dialog.getByText("Technical details").click();
    await expect(dialog.getByText(new URL(page.url()).origin, { exact: true })).toBeVisible();
    await expect(dialog.getByText("machine:read, session:read, pane:read, pane:input, message:send, pane:interrupt", { exact: true })).toBeVisible();
    await expect(dialog.getByText("Create a ten-minute code, then approve it on the machine you want to pair.")).toBeVisible();
    await expect(email).toBeVisible();
    await expect(createCode).toBeVisible();

    // 2. Fill Email me the code too (optional) with “not-an-email” and click Create pairing code.
    await email.fill("not-an-email");
    await createCode.click();
    expect(await email.evaluate((input) => (input as HTMLInputElement).type)).toBe("email");
    expect(await email.evaluate((input) => (input as HTMLInputElement).checkValidity())).toBe(false);
    expect(await email.evaluate((input) => (input as HTMLInputElement).validity.typeMismatch)).toBe(true);
    expect(await email.evaluate((input) => (input as HTMLInputElement).validationMessage)).not.toBe("");
    await expect(dialog.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
    await expect(createCode).toBeVisible();

    // 3. Replace the field with “operator@example.com”.
    await email.fill("operator@example.com");
    await expect(email).toHaveValue("operator@example.com");
    expect(await email.evaluate((input) => (input as HTMLInputElement).checkValidity())).toBe(true);
    await expect(createCode).toBeVisible();
    await expect(createCode).toBeEnabled();
  });
});
