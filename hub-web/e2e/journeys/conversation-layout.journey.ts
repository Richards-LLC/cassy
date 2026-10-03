import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";

for (const colorScheme of ["light", "dark"] as const) {
  test.describe(colorScheme, () => {
    test.use({ viewport: { width: 390, height: 800 }, colorScheme });
    test(`HUB-J9 hidden terminal retains its grid without widening the ${colorScheme} conversation cas_ff3d`, journeyPart, async ({ page, journey }, testInfo) => {
      const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
      await journey.stage("Read the phone conversation over the mounted terminal", async () => {
        await journey.open();
        await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
        const thread = page.locator(".conversation-reading.thread");
        const canvas = page.locator(".conversation-active > canvas");
        await expect(thread).toBeVisible();
        await expect(canvas).toBeAttached();
        await expect.poll(() => canvas.evaluate((node) => node.width), { message: "the real terminal has sized its 80-column backing store" }).toBeGreaterThan(390);
        hub.supervisorSays(PELICAN, "The phone conversation stays readable.");
        await expect(page.getByRole("log").getByText("The phone conversation stays readable.")).toBeVisible();
        const geometry = await page.locator(".terminal-mount.conversation-active").evaluate((mount) => {
          const box = (node: Element) => {
            const r = node.getBoundingClientRect();
            const s = getComputedStyle(node);
            return { tag: node.tagName, className: node.className, x: r.x, y: r.y, width: r.width, height: r.height, scrollWidth: node.scrollWidth, clientWidth: node.clientWidth, display: s.display, visibility: s.visibility, position: s.position, pointerEvents: s.pointerEvents };
          };
          const thread = mount.querySelector<HTMLElement>(".conversation-reading")!;
          const canvas = mount.querySelector<HTMLCanvasElement>("canvas")!;
          const r = thread.getBoundingClientRect();
          const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
          const before = { mount: box(mount), thread: box(thread), children: [...mount.children].map(box), hitInReadingSurface: !!hit && thread.contains(hit), backingWidth: canvas.width };
          const original = canvas.style.display;
          canvas.style.display = "none";
          const withoutCanvas = { mount: box(mount), thread: box(thread) };
          canvas.style.display = original;
          return { before, withoutCanvas, pageWidth: document.documentElement.scrollWidth, viewport: innerWidth };
        });
        mkdirSync(testInfo.outputDir, { recursive: true });
        writeFileSync(join(testInfo.outputDir, "geometry.json"), JSON.stringify(geometry, null, 2));
        expect(geometry.before.hitInReadingSurface, "the reader, not the hidden terminal, receives the hit").toBe(true);
        expect(geometry.before.thread.scrollWidth).toBeLessThanOrEqual(geometry.before.thread.clientWidth + 1);
        expect(geometry.pageWidth).toBeLessThanOrEqual(geometry.viewport);
        expect(geometry.before.mount.scrollWidth, "hidden terminal descendants do not widen the conversation mount").toBeLessThanOrEqual(geometry.before.mount.clientWidth + 1);
      });
    });
  });
}
