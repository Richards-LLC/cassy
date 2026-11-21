import type { Page } from "@playwright/test";
import { test, expect, journeyPart } from "./journey";
import { ATLAS, STUDIO, PELICAN } from "./world";

test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

// cas-5072: a phone turned sideways, notch on either side. Safari gives the
// page side insets (viewport-fit=cover) in a tab as well as installed, so
// every row, button, field and sheet control must sit clear of both bands.
// Chromium's CDP override supplies the insets to the production bundle.
// 844×390 shows Raw output as a bottom sheet; 932×430 (a larger phone) as the
// side drawer, which must not gain an empty left gutter (cas-5072 QA F01).
const SIDEWAYS = [{ width: 844, height: 390 }, { width: 932, height: 430 }] as const;
const NOTCH = 44;

async function notchInsets(page: Page): Promise<void> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { top: 0, left: NOTCH, right: NOTCH, bottom: 21 } });
}

/** Visible, reachable controls inside `scope` whose box enters a notch band. */
async function underTheBands(page: Page, scope: string): Promise<string[]> {
  const width = page.viewportSize()!.width;
  return page.locator(`${scope} :is(button, a[href], input, textarea, select, summary, [role="button"], [tabindex]:not([tabindex="-1"]))`).evaluateAll(
    (nodes, [width, notch]) =>
      nodes
        .filter((node) => {
          const element = node as HTMLElement;
          if (element.closest("[inert], [hidden], [aria-hidden='true']")) return false;
          if (!element.checkVisibility({ visibilityProperty: true, opacityProperty: true })) return false;
          // A focusable scroll region (the thread's log) spans the page; what it holds is checked itself.
          if (["auto", "scroll"].includes(getComputedStyle(element).overflowY)) return false;
          const box = element.getBoundingClientRect();
          // 1px clipped helpers (visually hidden text, skip links off-screen) are not targets.
          if (box.width <= 1 || box.height <= 1) return false;
          if (box.right <= 0 || box.left >= width || box.bottom <= 0 || box.top >= window.innerHeight) return false;
          return box.left < notch || box.right > width - notch;
        })
        .map((node) => {
          const element = node as HTMLElement;
          const box = element.getBoundingClientRect();
          const name = element.getAttribute("aria-label") || element.id || element.textContent?.trim().slice(0, 32) || element.tagName;
          return `${name} (left ${Math.round(box.left)}, right ${Math.round(box.right)})`;
        }),
    [width, NOTCH] as const,
  );
}

for (const { width, height } of SIDEWAYS) test.describe(`sideways with a notch ${width}x${height}`, () => {
  test.use({ viewport: { width, height }, hasTouch: true, isMobile: true, colorScheme: "dark" });

  test(`HUB-J9 sideways phone with a notch at ${width}x${height}: nothing to tap sits under the side bands (cas-5072)`, journeyPart, async ({ page, journey }) => {
    const hub = await journey.hub({ machines: [ATLAS, STUDIO], paired: ["atlas", "studio"] });
    await notchInsets(page);
    const list = page.getByRole("navigation", { name: "Choose a supervisor" });

    await journey.stage("Turn the phone sideways with the notch at one edge", async () => {
      await journey.open();
      await expect(list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row"))).toBeVisible();
      await expect(page.getByRole("button", { name: "Write to a supervisor" })).toBeVisible();
      expect(await underTheBands(page, ".conversation-sidebar"), "list rows, Write to a supervisor and the footer clear both notches").toEqual([]);
    });

    await journey.stage("Open a conversation that waits on me", async () => {
      await list.getByRole("button", { name: /cas-src/ }).and(list.locator(".conversation-row")).click();
      await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
      await hub.socketOpen(PELICAN);
      hub.supervisorSays(PELICAN, "Fix the warning in-train, or ship allowlisted?", { kind: "ask", options: ["Fix in-train", "Ship allowlisted"] });
      await expect(page.getByRole("region", { name: `Waiting on you: question from ${PELICAN}` })).toBeVisible();
      await page.getByRole("textbox", { name: "Your message" }).fill("Looking now");
      expect(await underTheBands(page, ".conversation-shell"), "the header, thread, Waiting-on-you strip, composer and Send clear both notches").toEqual([]);
    });

    await journey.stage("Read the raw output sideways", async () => {
      await page.getByRole("button", { name: "Raw output", exact: true }).click();
      const drawer = page.getByRole("dialog", { name: "Raw output" });
      await expect(drawer).toBeVisible();
      expect(await underTheBands(page, ".raw-output-drawer"), "the raw-output title and Close clear both notches").toEqual([]);
      const title = (await drawer.getByRole("heading", { name: "Raw output" }).boundingBox())!;
      expect(title.x, "the raw-output title clears the left notch").toBeGreaterThanOrEqual(NOTCH);
      const sheet = (await drawer.boundingBox())!;
      if (sheet.x > 0) {
        // The side drawer sits on the right edge, away from the left band: its
        // title keeps the ordinary padding, with no notch-width gutter.
        expect(title.x - sheet.x, "no empty left gutter on the side drawer").toBeLessThan(NOTCH);
      }
      await page.keyboard.press("Escape");
      await expect(drawer).toBeHidden();
    });
  });
});
