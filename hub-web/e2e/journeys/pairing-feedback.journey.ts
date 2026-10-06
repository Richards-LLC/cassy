import { test, expect, journeyPart } from "./journey";
import { ATLAS } from "./world";
import { showConversationList } from "./responsive-goals";
import type { Page, TestInfo, Route } from "@playwright/test";
import { writeFileSync } from "node:fs";

test.describe.configure({ mode: "parallel" });

async function captureRendered(page: Page, info: TestInfo, name: string): Promise<void> {
  await page.screenshot({ path: info.outputPath(`${name}.png`) });
  const html = await page.evaluate(() => {
    const clone = document.documentElement.cloneNode(true) as HTMLElement;
    clone.querySelectorAll("script, link").forEach(node => node.remove());
    return clone.outerHTML;
  });
  const css = await (await page.request.get(new URL("app.css", page.url()).href)).text();
  writeFileSync(info.outputPath(`${name}.html`), "<!doctype html>" + html.replace("</head>", `<style>${css}</style></head>`));
}

for (const width of [390, 1280]) {
  for (const scheme of ["light", "dark"] as const) {
    test.describe(`${width} ${scheme}`, () => {
      test.use({ viewport: { width, height: width === 390 ? 844 : 800 }, hasTouch: width === 390, isMobile: width === 390, colorScheme: scheme });

      test(`HUB-J2 timeout feedback in view ${width} ${scheme} (cas-2e77)`, journeyPart, async ({ page, journey }, info) => {
        const hub = await journey.hub({ machines: [ATLAS] });
        let blocked = true;
        let held: Route | undefined;
        await page.route("https://atlas.test/v1/auth/pairing/exchange", route => {
          if (blocked) { held = route; return; }
          return route.fallback();
        });
        const dialog = page.locator("#pair-dialog");
        await journey.stage("A timed-out pairing puts the cause and next action in view", async () => {
          await page.goto(`./#pair=${"t".repeat(43)}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=machine:read,session:read,pane:read`);
          await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Operator");
          await dialog.getByRole("button", { name: "Pair", exact: true }).click();
          await expect.poll(() => Boolean(held)).toBe(true);
          await page.clock.runFor(10_001);
          const feedback = dialog.locator(".pair-status");
          await expect(feedback).toContainText("Pairing timed out after 10s");
          await expect(feedback).toContainText("Allow Local network access");
          await expect(feedback).toContainText("Pair again");
          await expect(feedback).toContainText("press Pair again");
          await expect(feedback).not.toContainText("tap Pair");
          await expect(feedback).toBeInViewport({ ratio: 1 });
          await expect(feedback).toBeFocused();
          await expect(feedback).toMatchAriaSnapshot('- alert: /Pairing timed out after 10s/');
          const geometry = await dialog.evaluate(el => {
            const advice = el.querySelector(".pair-status")!;
            const feedback = advice.getBoundingClientRect();
            const text = document.createRange(); text.selectNodeContents(advice);
            const glyphLeft = Math.min(...Array.from(text.getClientRects(), rect => rect.left));
            const outlineInset = Math.max(0, -parseFloat(getComputedStyle(advice).outlineOffset));
            const form = el.querySelector("form")!.getBoundingClientRect();
            const actions = el.querySelector(".dialog-actions")!.getBoundingClientRect();
            return { top: feedback.top, bottom: feedback.bottom, formTop: form.top, actionsTop: actions.top, glyphClearance: glyphLeft - feedback.left - outlineInset };
          });
          writeFileSync(info.outputPath("feedback-bounds.json"), JSON.stringify(geometry, null, 2) + "\n");
          expect(geometry.top).toBeGreaterThanOrEqual(geometry.formTop - 0.5);
          expect(geometry.bottom, "the sticky actions do not cover the advice").toBeLessThanOrEqual(geometry.actionsTop + 0.5);
          expect(geometry.glyphClearance, "the inset focus ring clears the advice's first glyphs").toBeGreaterThanOrEqual(2);
          await expect(dialog.getByRole("button", { name: "Pair", exact: true })).toBeEnabled();
          expect(hub.exchanges).toHaveLength(0);
          await captureRendered(page, info, "timeout");
        });
        await journey.stage("Retry uses the retained invitation and completes once", async () => {
          await held!.abort("failed").catch(() => {});
          held = undefined;
          await dialog.getByRole("button", { name: "Pair", exact: true }).click();
          await expect.poll(() => Boolean(held)).toBe(true);
          const feedback = dialog.locator(".pair-status");
          await expect(feedback).toHaveAttribute("role", "status");
          await expect(feedback).toContainText("Updating this browser installation");
          expect(await dialog.evaluate(el => el.contains(document.activeElement)), "retry focus remains in the dialog").toBe(true);
          blocked = false;
          await held!.fallback();
          await expect(dialog).toBeHidden();
          expect(hub.exchanges).toHaveLength(1);
        });
      });

      test(`HUB-J2 human installation identity ${width} ${scheme} (cas-2e77)`, journeyPart, async ({ page, journey }, info) => {
        const hub = await journey.hub({ machines: [ATLAS] });
        await journey.stage("Pair a browser with a recognizable name", async () => {
          await page.goto(`./#pair=${"i".repeat(43)}&hub=atlas&hub_url=https%3A%2F%2Fatlas.test&machine=Atlas&scopes=machine:read,session:read,pane:read`);
          const dialog = page.locator("#pair-dialog");
          await dialog.getByRole("textbox", { name: "Your name (shown on the machine)" }).fill("Daniel");
          const browserName = dialog.getByRole("textbox", { name: "Name for this browser" });
          await browserName.fill("   ");
          await dialog.getByRole("button", { name: "Pair", exact: true }).click();
          await expect(browserName).toBeFocused();
          expect(await browserName.evaluate(el => (el as HTMLInputElement).validity.valid)).toBe(false);
          expect(hub.exchanges).toHaveLength(0);
          await browserName.fill("  Work laptop  ");
          await dialog.getByRole("button", { name: "Pair", exact: true }).click();
          await expect(dialog).toBeHidden();
          expect([...hub.installations.values()][0]!.device_label).toBe("Work laptop");
        });
        await journey.stage("Recognize this browser without reading protocol fields", async () => {
          await showConversationList(page);
          await page.locator("#paired-machines-toggle").click();
          await page.getByRole("button", { name: "Browser installations on Atlas", exact: true }).click();
          const row = page.locator(".installation-inventory-row");
          await expect(row.getByRole("heading", { name: "Work laptop · This browser", exact: true })).toBeVisible();
          const visible = await row.innerText();
          expect(visible).toContain("Daniel");
          expect(visible).toContain("Active");
          expect(visible).not.toMatch(/Generation|Signing key|key epoch|\d{4}-\d{2}-\d{2}T/);
          expect(visible).not.toContain([...hub.installations.keys()][0]!);
          await expect(row.locator("time")).toHaveCount(2);
          for (const time of await row.locator("time").all()) {
            await expect(time).toHaveAttribute("title", /.+/);
            await expect(time).not.toHaveText(/\d{4}-\d{2}-\d{2}T/);
            await expect(time).toHaveText("Just now");
          }
          await expect(row.getByRole("heading")).toMatchAriaSnapshot('- heading "Work laptop · This browser" [level=3]');
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
          await captureRendered(page, info, "inventory");
        });
        await journey.stage("Inspect exact technical identity from the keyboard", async () => {
          const row = page.locator(".installation-inventory-row");
          const summary = row.locator("summary");
          await summary.focus();
          await page.keyboard.press("Enter");
          const technical = row.locator("details");
          await expect(technical).toHaveAttribute("open", "");
          await expect(technical).toContainText([...hub.installations.keys()][0]!);
          await expect(technical.getByText("Signing key", { exact: true })).toBeVisible();
          await summary.press("Enter");
          await expect(technical).not.toHaveAttribute("open", "");
          await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
          await expect(page.getByRole("button", { name: "Browser installations on Atlas", exact: true })).toBeFocused();
          const longName = "LongBrowserName".repeat(7);
          [...hub.installations.values()][0]!.device_label = longName;
          await page.getByRole("button", { name: "Browser installations on Atlas", exact: true }).click();
          const longHeading = page.locator(".installation-inventory-row h3");
          await expect(longHeading).toHaveText(`${longName} · This browser`);
          const bounds = await longHeading.evaluate(el => {
            const heading = el.getBoundingClientRect();
            const row = el.closest(".installation-inventory-row")!.getBoundingClientRect();
            const sheet = el.closest("dialog")!.getBoundingClientRect();
            return { headingRight: heading.right, rowRight: row.right, sheetRight: sheet.right };
          });
          expect(bounds.headingRight).toBeLessThanOrEqual(bounds.sheetRight);
          expect(bounds.rowRight).toBeLessThanOrEqual(bounds.sheetRight);
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
          await page.screenshot({ path: info.outputPath("inventory-long.png") });
          await page.locator(".installation-inventory").getByRole("button", { name: "Close", exact: true }).click();
        });
      });
    });
  }
}
