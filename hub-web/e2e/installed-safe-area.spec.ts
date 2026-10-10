import { test, expect, type Page } from "@playwright/test";

// cas-8951 QA F01/F02: installed on an iPhone (display: standalone with the
// black-translucent status bar), the page draws under the status bar and
// env(safe-area-inset-top) is about 47px. Every top-anchored surface on a
// phone must keep its controls below that inset. Chromium's CDP override
// supplies the inset on the fixture site (real src/ builders and styles).
const STATUS_BAR = 47;
const NOTCH = 44; // landscape: the notch side inset

async function installedInsets(page: Page, insets: { top: number; left?: number; right?: number }) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("Emulation.setSafeAreaInsetsOverride", { insets: { top: insets.top, left: insets.left ?? 0, right: insets.right ?? 0, bottom: 34 } });
}

async function controlBoxes(page: Page, surface: string) {
  const boxes = await page.locator(`${surface} :is(button, a, h1, h2, input, .cloud-brand)`).evaluateAll((nodes) =>
    nodes
      .filter((node) => (node as HTMLElement).offsetParent !== null && node.getBoundingClientRect().width > 0)
      .map((node) => {
        const box = node.getBoundingClientRect();
        return { label: (node as HTMLElement).id || node.textContent?.trim().slice(0, 32) || node.tagName, top: box.top, left: box.left, right: box.right };
      }));
  expect(boxes.length, `${surface} has visible controls`).toBeGreaterThan(0);
  return boxes;
}

for (const [fixture, surface] of [
  ["conversations-list", ".conversation-list-heading"],
  ["conversation", ".conversation-heading"],
  ["launch-form", ".launch-sheet"],
] as const) {
  test(`installed iPhone: ${surface} clears the status bar (${fixture})`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await installedInsets(page, { top: STATUS_BAR });
    await page.goto(`/?fixture=${fixture}`);
    await expect(page.locator(surface).first()).toBeVisible();
    for (const box of await controlBoxes(page, surface)) {
      expect(box.top, `${box.label} sits below the ${STATUS_BAR}px status bar`).toBeGreaterThanOrEqual(STATUS_BAR);
    }
  });
}

test("installed iPhone, landscape: the list header clears the notch", async ({ page }) => {
  await page.setViewportSize({ width: 844, height: 390 });
  await installedInsets(page, { top: 0, left: NOTCH, right: NOTCH });
  await page.goto("/?fixture=conversations-list");
  const width = 844;
  for (const box of await controlBoxes(page, ".conversation-list-heading")) {
    expect(box.left, `${box.label} clears the left notch`).toBeGreaterThanOrEqual(NOTCH);
    expect(box.right, `${box.label} clears the right notch`).toBeLessThanOrEqual(width - NOTCH);
  }
});

test("in a browser tab (no inset) the headers keep their geometry", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/?fixture=conversation");
  const top = await page.locator(".conversation-heading #conversation-back").evaluate((node) => node.getBoundingClientRect().top);
  expect(top).toBeLessThan(STATUS_BAR);
});
