import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";

for (const colorScheme of ["light", "dark"] as const) {
  test.describe(colorScheme, () => {
    test.use({ viewport: { width: 390, height: 800 }, colorScheme });
    test(`HUB-J9 the hidden pane host never widens or covers the ${colorScheme} conversation cas_ff3d`, journeyPart, async ({ page, journey }, testInfo) => {
      const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
      await journey.stage("Read the phone conversation beside the hidden pane host", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        const thread = page.locator(".conversation-reading.thread");
        await expect(thread).toBeVisible();
        // cas-0546: the supervisor pane's surface is attached for Raw output,
        // inside a host that is hidden, inert and out of the accessibility
        // tree, beside the thread's slot and never around it.
        const host = page.locator("#pane-grid > .pane-host");
        await expect(host).toBeAttached();
        await expect(host).toBeHidden();
        await expect(host).toHaveAttribute("inert", "");
        await expect(host).toHaveAttribute("aria-hidden", "true");
        await expect(page.locator("#pane-grid > .conversation-thread-slot")).toBeVisible();
        await expect(host.locator(".terminal-mount")).toBeAttached();
        hub.supervisorSays(PELICAN, "The phone conversation stays readable.");
        await expect(page.getByRole("log").getByText("The phone conversation stays readable.")).toBeVisible();
        expect(await page.locator("body").ariaSnapshot(), "no terminal in the accessibility tree").not.toContain("The supervisor is ready.");
        const geometry = await page.locator("#pane-grid > .conversation-thread-slot").evaluate((slot) => {
          const box = (node: Element) => {
            const r = node.getBoundingClientRect();
            const s = getComputedStyle(node);
            return { tag: node.tagName, className: node.className, x: r.x, y: r.y, width: r.width, height: r.height, scrollWidth: node.scrollWidth, clientWidth: node.clientWidth, display: s.display, visibility: s.visibility, position: s.position, pointerEvents: s.pointerEvents };
          };
          const thread = slot.querySelector<HTMLElement>(".conversation-reading")!;
          const host = slot.parentElement!.querySelector<HTMLElement>(":scope > .pane-host")!;
          const r = thread.getBoundingClientRect();
          const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
          return { slot: box(slot), thread: box(thread), host: box(host), grid: box(slot.parentElement!), hitInReadingSurface: !!hit && thread.contains(hit), pageWidth: document.documentElement.scrollWidth, viewport: innerWidth };
        });
        mkdirSync(testInfo.outputDir, { recursive: true });
        writeFileSync(join(testInfo.outputDir, "geometry.json"), JSON.stringify(geometry, null, 2));
        expect(geometry.hitInReadingSurface, "the reader, not the pane host, receives the hit").toBe(true);
        expect(geometry.host.display, "the pane host renders nothing").toBe("none");
        expect(geometry.thread.scrollWidth).toBeLessThanOrEqual(geometry.thread.clientWidth + 1);
        expect(geometry.pageWidth).toBeLessThanOrEqual(geometry.viewport);
        expect(geometry.grid.scrollWidth, "the pane host does not widen the conversation").toBeLessThanOrEqual(geometry.grid.clientWidth + 1);
        expect(geometry.slot.scrollWidth, "nothing in the thread slot overflows it").toBeLessThanOrEqual(geometry.slot.clientWidth + 1);
      });
      await journey.stage("Open Raw output and return to the same readable conversation", async () => {
        const raw = page.getByRole("button", { name: "Raw output", exact: true });
        await raw.click();
        const drawer = page.getByRole("dialog", { name: "Raw output" });
        await expect(drawer).toBeVisible();
        await expect(drawer.getByRole("log", { name: "Raw output" })).toContainText("The supervisor is ready.");
        // On a phone it is a bottom sheet across the width, inside the page.
        const sheet = (await drawer.boundingBox())!;
        expect(Math.round(sheet.x + sheet.width), "the sheet spans the width").toBe(390);
        expect(Math.round(sheet.y + sheet.height), "the sheet sits on the bottom edge").toBe(800);
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
        await drawer.getByRole("button", { name: "Close raw output" }).click();
        await expect(drawer).toBeHidden();
        await expect(raw).toBeFocused();
        await expect(page.getByRole("log").getByText("The phone conversation stays readable.")).toBeVisible();
        await expect(page.locator(".pane-host")).toBeHidden();
        const grid = page.locator("#pane-grid");
        expect(await grid.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(true);
      });
    });
  });
}
