// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

import { test, expect } from "@playwright/test";

test.describe("Attention panel", () => {
  test("Attention panel distinguishes critical, warning, and info events", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=attention-12.
    await page.goto("/?fixture=attention-12");
    const attentionTab = page.getByRole("tab", { name: "Attention" });
    await expect(attentionTab).toHaveAttribute("aria-selected", "true");
    await expect(page.getByText("12 events need attention")).toBeVisible();
    const sessionGroup = page.getByRole("button", { name: /critical commander-session-with-a-long-codename 12/ });
    await expect(sessionGroup).toBeVisible();
    await expect(sessionGroup).toHaveAttribute("aria-expanded", "true");
    await expect(sessionGroup).toContainText("12");

    // 2. Inspect the Daemon connection lost article, a Connection attempt warning article, and the Connection attempt 4 info article.
    const critical = page.getByRole("article").filter({ has: page.getByText("Daemon connection lost", { exact: true }) });
    const warning = page.getByRole("article").filter({ has: page.getByText("Connection attempt 2", { exact: true }) });
    const info = page.getByRole("article").filter({ has: page.getByText("Connection attempt 4", { exact: true }) });

    await expect(critical).toHaveCount(1);
    await expect(warning).toHaveCount(1);
    await expect(info).toHaveCount(1);
    await expect(critical).toHaveClass(/attention-item--critical/);
    await expect(warning).toHaveClass(/attention-item--warning/);
    await expect(info).toHaveClass(/attention-item--info/);

    await expect(critical.getByRole("button", { name: "Retry" })).toBeVisible();
    await expect(critical.getByRole("button", { name: "Dismiss critical event" })).toBeVisible();
    await expect(warning.getByRole("button", { name: "Retry" })).toBeVisible();
    await expect(warning.getByRole("button", { name: "Dismiss warning event" })).toBeVisible();
    await expect(info.getByRole("button", { name: "Dismiss info event" })).toBeVisible();
    await expect(info.getByRole("button", { name: "Retry" })).toHaveCount(0);
  });
});
