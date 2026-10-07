import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { writeFileSync } from "node:fs";

// Exercise the production bundle through the same list -> conversation route
// as the retained cas-0546 F02/F03 traces. Each part has independent receipts.
test.describe.configure({ mode: "parallel" });

const cells = [
  { id: "M01", name: "Atlas phone light", scheme: "light", width: 390 },
  { id: "M02", name: "Atlas phone dark", scheme: "dark", width: 390 },
  { id: "M03", name: "desktop light identity", scheme: "light", width: 1280 },
  { id: "M04", name: "desktop dark identity", scheme: "dark", width: 1280 },
  { id: "M05", name: "phone forced colors", scheme: "light", width: 390, forcedColors: "active" },
  { id: "M06", name: "phone reduced motion", scheme: "dark", width: 390, reducedMotion: "reduce" },
  { id: "M07", name: "phone increased contrast", scheme: "light", width: 390, contrast: "more" },
  { id: "M08", name: "long machine keyboard revisit", scheme: "dark", width: 390, long: true },
] as const;

for (const cell of cells) {
  test.describe(cell.name, () => {
    test.use({
      viewport: { width: cell.width, height: cell.width === 390 ? 844 : 800 },
      hasTouch: cell.width === 390,
      isMobile: cell.width === 390,
      colorScheme: cell.scheme,
    });

    test(`HUB-J3 host glyph ${cell.id} ${cell.name} (cas-9412)`, journeyPart, async ({ page, journey }, testInfo) => {
      const label = "long" in cell ? "Atlas-with-a-very-long-machine-name-for-an-operator · Linux" : ATLAS.label;
      await page.emulateMedia({
        forcedColors: "forcedColors" in cell ? cell.forcedColors : "none",
        reducedMotion: "reducedMotion" in cell ? cell.reducedMotion : "no-preference",
        contrast: "contrast" in cell ? cell.contrast : "no-preference",
      });
      await journey.hub({ machines: [{ ...ATLAS, label }], paired: [ATLAS.id] });

      await journey.stage("Choose cas-src and read its machine", async () => {
        await journey.open();
        const row = page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ });
        if (cell.width === 390) await row.tap(); else await row.click();
        await expect(page.locator(".conversation-identity h1")).toMatchAriaSnapshot('- heading "cas-src" [level=1]');
        await expect(page.locator(".conversation-identity .host-machine")).toBeVisible();
      });

      if ("long" in cell) {
        await journey.stage("Return to the list and reopen with the keyboard", async () => {
          await page.getByRole("button", { name: "‹ Conversations", exact: true }).tap();
          const search = page.getByRole("searchbox", { name: "Search conversations" });
          await search.focus();
          await search.fill("cas-src");
          await page.keyboard.press("Enter");
          const focus = await page.evaluate(() => ({
            tag: document.activeElement?.tagName,
            id: document.activeElement?.id,
            classes: document.activeElement?.className,
            label: document.activeElement?.getAttribute("aria-label"),
            finePointer: matchMedia("(pointer: fine)").matches,
          }));
          const focusPath = testInfo.outputPath("revisit-focus.json");
          writeFileSync(focusPath, JSON.stringify(focus, null, 2) + "\n");
          await testInfo.attach("revisit focus", { path: focusPath, contentType: "application/json" });
          // Phone navigation lands on the reading region, so reopening the
          // conversation does not raise the soft keyboard (cas-12c29).
          await expect(page.locator(".conversation-reading.thread")).toBeFocused();
          await expect(page.getByRole("textbox", { name: "Your message" })).not.toBeFocused();
        });
      }

      await journey.stage("The host identity is whole vertically and fits the page", async () => {
        const line = page.locator(".conversation-identity .host-where");
        await expect(line).toHaveAttribute("title", `${label} · ${PELICAN}`);
        // Dropped OS and codename remain in the accessible text, even though
        // their intentionally sr-only rectangles cannot represent ink bounds.
        await expect(line).toContainText(label);
        await expect(line).toContainText(PELICAN);
        await page.evaluate(() => document.fonts.ready);
        const bounds = await page.locator(".conversation-identity .host-machine").evaluate(el => {
          const text = Array.from(el.childNodes).find(node => node.nodeType === Node.TEXT_NODE && node.textContent?.trim());
          if (!text) throw new Error("The machine has no visible name text");
          const range = document.createRange();
          range.selectNodeContents(text);
          const rect = (r: DOMRect) => ({ x: r.x, y: r.y, width: r.width, height: r.height, top: r.top, bottom: r.bottom });
          const clips = [];
          for (let ancestor: Element | null = el; ancestor; ancestor = ancestor.parentElement) {
            const style = getComputedStyle(ancestor);
            if (/hidden|clip|auto|scroll/.test(style.overflowY)) {
              const box = ancestor.getBoundingClientRect();
              clips.push({
                selector: ancestor.className,
                top: box.top + parseFloat(style.borderTopWidth),
                bottom: box.bottom - parseFloat(style.borderBottomWidth),
              });
            }
          }
          const style = getComputedStyle(el);
          return {
            text: rect(range.getBoundingClientRect()), element: rect(el.getBoundingClientRect()), clips,
            font: style.font, lineHeight: style.lineHeight, fontSize: style.fontSize,
            hostClass: el.parentElement!.className,
            machineEllipsises: el.scrollWidth > el.clientWidth + 1,
            pageFits: document.documentElement.scrollWidth <= innerWidth,
            media: {
              forcedColors: matchMedia("(forced-colors: active)").matches,
              reducedMotion: matchMedia("(prefers-reduced-motion: reduce)").matches,
              contrast: matchMedia("(prefers-contrast: more)").matches,
            },
          };
        });
        const geometry = testInfo.outputPath("host-bounds.json");
        writeFileSync(geometry, JSON.stringify({ cell: cell.id, label, ...bounds }, null, 2) + "\n");
        await testInfo.attach("host bounds", { path: geometry, contentType: "application/json" });
        expect(bounds.pageFits, "the machine does not widen the phone or desktop").toBe(true);
        expect(bounds.media).toEqual({ forcedColors: "forcedColors" in cell, reducedMotion: "reducedMotion" in cell, contrast: "contrast" in cell });
        if (cell.width === 390) {
          expect(bounds.clips.length, "measure the real clipping ancestors").toBeGreaterThan(0);
          for (const clip of bounds.clips) {
            expect(bounds.text.top, `${clip.selector} must not clip the glyph tops`).toBeGreaterThanOrEqual(clip.top - 0.05);
            expect(bounds.text.bottom, `${clip.selector} must not clip the glyph bottoms`).toBeLessThanOrEqual(clip.bottom + 0.05);
          }
          await expect(page.getByRole("button", { name: "‹ Conversations", exact: true })).toBeVisible();
        } else {
          // The change is restricted to <=500px. Record desktop geometry but
          // retain its existing typography and navigation contract.
          expect(bounds.fontSize).toBe("13px");
          expect(bounds.lineHeight).toBe("16.25px");
          await expect(page.getByRole("navigation", { name: "Choose a supervisor" })).toBeVisible();
        }
        if ("long" in cell) {
          expect(bounds.machineEllipsises, "long machine names still ellipsise horizontally").toBe(true);
          // cas-d043 G01: the identity has its own row, so a 51-character
          // machine keeps 30-odd characters and the codename its 8ch, where
          // beside the actions the codename had to step aside entirely.
          expect(bounds.hostClass).toContain("machine-long");
          expect(bounds.hostClass).not.toContain("codename-squeezed");
        }
      });
    });
  });
}
