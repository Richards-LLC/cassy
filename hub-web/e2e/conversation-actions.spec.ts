import { test, expect } from "@playwright/test";

// cas-0546: the conversation header's Interrupt and Raw output, on the
// fixture site (real src/ builders, fixture data, no hub).

test("Interrupt names why it can't run while Raw output stays available", async ({ page }) => {
  await page.goto("/?fixture=conversation-interrupt-unavailable");
  const interrupt = page.locator("#conversation-interrupt");
  const rawOutput = page.locator("#conversation-raw-output");
  await expect(interrupt).toBeVisible();
  await expect(interrupt).toHaveAttribute("aria-disabled", "true");
  await expect(interrupt).toHaveAccessibleName("Interrupt the cas-src supervisor");
  await expect(interrupt).toHaveAccessibleDescription(/paired without permission to interrupt/);
  await expect(rawOutput).not.toHaveAttribute("aria-disabled");
});

test("A revoked pairing leaves both header actions focusable with the pairing reason", async ({ page }) => {
  await page.goto("/?fixture=conversation-needs-pairing");
  for (const id of ["#conversation-raw-output", "#conversation-interrupt"]) {
    const action = page.locator(id);
    await expect(action).toHaveAttribute("aria-disabled", "true");
    await expect(action).toHaveAccessibleDescription(/Atlas · Linux/);
    await action.focus();
    await expect(action).toBeFocused();
  }
});

test("The Raw output drawer opens read-only over the conversation and closes", async ({ page }) => {
  await page.goto("/?fixture=conversation-raw-output");
  const drawer = page.getByRole("dialog", { name: "Raw output" });
  await expect(drawer).toBeVisible();
  await expect(page.locator("dialog#raw-output:modal")).toBeVisible();
  await expect(drawer).toContainText("What the cas-src supervisor's terminal shows, as text. Read-only.");
  await expect(drawer.getByRole("log", { name: "Raw output" })).toContainText("# Build result");
  await expect(drawer.getByRole("textbox")).toHaveCount(0);
  await expect(page.locator("#conversation-raw-output")).toHaveAttribute("aria-expanded", "true");
  await drawer.getByRole("button", { name: "Close raw output" }).click();
  await expect(drawer).toBeHidden();
});

test("A lost connection keeps the thread under the banner and says why the actions wait", async ({ page }) => {
  await page.goto("/?fixture=connection-fatal-browser");
  const banner = page.locator("#pane-grid > .terminal-disconnected-banner");
  await expect(banner).toBeVisible();
  await expect(banner).toContainText("Lost connection to Atlas · Linux.");
  await expect(page.getByRole("log")).toContainText("The project badge stays visible");
  await expect(page.locator("#conversation-interrupt")).toHaveAccessibleDescription(/Interrupt and raw output wait until then\./);
});
