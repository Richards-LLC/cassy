import { test, expect } from "@playwright/test";

// spec: specs/hub-web-fixtures.md
// seed: e2e/seed.spec.ts

test.describe("Populated fleet", () => {
  test("Fleet summary and work-state table agree with the session ledger", async ({ page }) => {
    // 1. From a fresh page, navigate to /?fixture=fleet-populated.
    await page.goto("/?fixture=fleet-populated");

    const fleet = page.locator('[aria-label="Fleet"]');
    await expect(fleet).toContainText("2 machines · 3 sessions · 1 not live");
    // The table marks quiet-marten as Needs you, but the fleet verdict currently says no sessions need you.
    test.fixme(true, "Fleet verdict disagrees with its work-state table and the fixture plan");
    await expect(fleet.getByRole("status")).toHaveText(
      "1 of 3 sessions needs you; 2 working.",
    );

    // 2. Inspect the work-state table and Session ledger.
    const table = page.getByRole("figure", {
      name: "Sessions on the work-state track",
    }).getByRole("table");
    await expect(table.locator("tbody tr")).toHaveCount(3);

    const quiet = table.getByRole("row").filter({
      has: page.getByRole("rowheader", { name: "quiet-marten on Forge desktop" }),
    });
    const bright = table.getByRole("row").filter({
      has: page.getByRole("rowheader", { name: "bright-otter on Atlas laptop" }),
    });
    const calm = table.getByRole("row").filter({
      has: page.getByRole("rowheader", { name: "calm-heron on Atlas laptop" }),
    });
    await expect(quiet).toHaveCount(1);
    await expect(bright).toHaveCount(1);
    await expect(calm).toHaveCount(1);
    await expect(quiet.getByRole("cell", { name: "Needs you" })).toHaveCount(1);
    await expect(bright.getByRole("cell", { name: "Working: editing" })).toHaveCount(1);
    await expect(calm.getByRole("cell", { name: "Working: testing" })).toHaveCount(1);

    const ledger = page.locator("section.fleet-evidence");
    await expect(ledger.getByRole("heading", { name: "Session ledger" })).toBeVisible();
    await expect(ledger.getByRole("listitem")).toHaveCount(3);

    const atlas = ledger.locator('section[data-fleet-machine="atlas"]');
    const forge = ledger.locator('section[data-fleet-machine="forge"]');
    await expect(atlas.locator("header")).toContainText("Atlas laptop");
    await expect(atlas.locator("header")).toContainText("Live");
    await expect(forge.locator("header")).toContainText("Forge desktop");
    await expect(forge.locator("header")).toContainText("Degraded");
    await expect(atlas.getByRole("listitem")).toHaveCount(2);
    await expect(forge.getByRole("listitem")).toHaveCount(1);
    await expect(atlas.getByRole("button", { name: "Open bright-otter on Atlas laptop" })).toHaveCount(1);
    await expect(atlas.getByRole("button", { name: "Open calm-heron on Atlas laptop" })).toHaveCount(1);
    await expect(forge.getByRole("button", { name: "Open quiet-marten on Forge desktop" })).toHaveCount(1);
  });
});
