import { test, expect } from "./journey";
import { ATLAS, PELICAN } from "./world";

test("HUB-J10 switch to dark and keep reading", async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });

  await journey.stage("Open the conversation in the light theme", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    hub.supervisorSays(PELICAN, "Status: the Linux lane is green; the Mac lane is still running.", { kind: "status" });
  });

  await journey.stage("Choose the dark appearance", async () => {
    // Ctrl+K lands in the list search; the appearance lives behind the list's Appearance & commands button.
    await page.getByRole("button", { name: "Appearance & commands" }).click();
    // The row in effect says so (F20): System, with a check and "Current".
    const current = page.locator("#command-palette [data-palette-scheme][aria-current='true']");
    await expect(current).toHaveCount(1);
    await expect(current).toHaveAttribute("data-palette-scheme", "system");
    await expect(current.locator(".palette-check")).toBeVisible();
    await expect(current.locator("small")).toHaveText("Current · follows this device");
    await page.getByRole("button", { name: /^Appearance · Dark/ }).click();
    await expect(page.locator("html")).toHaveAttribute("data-scheme", "dark");
  });

  await journey.stage("Keep reading in dark", async () => {
    await expect(page.getByRole("log").getByText("Status: the Linux lane is green; the Mac lane is still running.")).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Your message" })).toBeVisible();
  });

  await journey.stage("The choice survives a reload", async () => {
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("data-scheme", "dark");
    await expect(page.locator("#command-palette [data-palette-scheme='dark']")).toHaveAttribute("aria-current", "true");
    await expect(page.locator("#command-palette [data-palette-scheme='dark'] small")).toHaveText("Current");
    await expect(page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true })).toBeVisible();
    await expect(page.getByRole("log").getByText("Status: the Linux lane is green; the Mac lane is still running.")).toBeVisible();
  });

  await journey.stage("High contrast keeps the open conversation and Send marked", async () => {
    // Windows high contrast (forced colours) drops author tints and fills; the
    // open row must still read as selected and Send as a button (cas-ac7d).
    const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
    const send = page.getByRole("button", { name: `Send to ${PELICAN}`, exact: true });
    const sidebar = page.locator(".conversation-sidebar");
    const bg = (locator: typeof row) => locator.evaluate((element) => getComputedStyle(element).backgroundColor);
    for (const colorScheme of ["dark", "light"] as const) {
      await page.emulateMedia({ colorScheme, forcedColors: "active" });
      expect(await page.evaluate(() => matchMedia("(forced-colors: active)").matches)).toBe(true);
      await expect(row).toHaveAttribute("aria-current", "true");
      expect(await bg(row), `${colorScheme}: the open row is filled`).not.toBe(await bg(sidebar));
      expect(await bg(row), `${colorScheme}: the open row fill is opaque`).not.toMatch(/rgba\(.*, 0\)|transparent/);
      const sendStyle = await send.evaluate((element) => { const style = getComputedStyle(element); return { background: style.backgroundColor, border: style.borderTopStyle, color: style.color }; });
      expect(sendStyle.border, `${colorScheme}: Send has an edge`).toBe("solid");
      expect(sendStyle.background, `${colorScheme}: Send is filled`).not.toBe(sendStyle.color);
      expect(sendStyle.background).not.toMatch(/rgba\(.*, 0\)|transparent/);
      // Pointer and keyboard on the open row keep its fill (a hover or focus
      // rule must not swap in the author tint under HighlightText).
      const filled = await bg(row);
      await row.hover();
      expect(await bg(row), `${colorScheme}: the open row keeps its fill under the pointer`).toBe(filled);
      await row.focus();
      expect(await bg(row), `${colorScheme}: the open row keeps its fill with focus`).toBe(filled);
      await page.mouse.move(700, 300);
    }
    await page.emulateMedia({ colorScheme: "dark", forcedColors: null });
  });
});
