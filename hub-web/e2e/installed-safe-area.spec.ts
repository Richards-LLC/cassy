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

test("landscape pairing code keeps its content scrollable and every action reachable", async ({ page }) => {
  await page.setViewportSize({ width: 844, height: 390 });
  await page.goto("/?fixture=pairing-code");
  const dialog = page.getByRole("dialog", { name: "Pair a machine", exact: true });
  await expect(dialog).toBeInViewport({ ratio: 1 });
  expect(await dialog.evaluate((node) => {
    const overflow = getComputedStyle(node).overflowY;
    return node.scrollHeight <= node.clientHeight + 1 || overflow === "auto" || overflow === "scroll";
  }), "pairing content must fit the dialog or remain scrollable, never clipped").toBe(true);

  for (const name of ["Copy command", "Send the code", "Cancel"]) {
    const action = dialog.getByRole("button", { name, exact: true });
    await action.focus();
    await expect(action).toBeFocused();
    await expect(action).toBeInViewport({ ratio: 1 });
    const [surface, control] = await Promise.all([dialog.boundingBox(), action.boundingBox()]);
    expect(control!.y, `${name} clears the dialog's top`).toBeGreaterThanOrEqual(surface!.y);
    expect(control!.y + control!.height, `${name} clears the dialog's bottom`).toBeLessThanOrEqual(surface!.y + surface!.height);
  }
});

test("in a browser tab (no inset) the headers keep their geometry", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/?fixture=conversation");
  const top = await page.locator(".conversation-heading #conversation-back").evaluate((node) => node.getBoundingClientRect().top);
  expect(top).toBeLessThan(STATUS_BAR);
});

test("conversation header shares one row with reachable actions (cas-bcf0)", async ({ page }, testInfo) => {
  for (const width of [1280, 390]) for (const colorScheme of ["light", "dark"] as const) {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 800 });
    await page.emulateMedia({ colorScheme });
    await page.goto("/?fixture=conversation");
    await page.locator(".conversation-shell").evaluate(node => node.classList.add("context-open"));
    const header = page.locator(".conversation-heading");
    await expect(header).toBeVisible();
    await header.screenshot({ path: testInfo.outputPath(`header-${width}-${colorScheme}.png`) });
    const boxes = await header.evaluate(node => {
      const rect = (selector: string) => {
        const box = node.querySelector(selector)!.getBoundingClientRect();
        return { x: box.x, right: box.right, y: box.y, width: box.width, height: box.height, middle: box.y + box.height / 2 };
      };
      return { header: node.getBoundingClientRect().height, identity: rect(".conversation-identity"), raw: rect("#conversation-raw-output"), interrupt: rect("#conversation-interrupt"), title: rect("h1 b") };
    });
    expect(Math.abs(boxes.identity.middle - boxes.raw.middle)).toBeLessThanOrEqual(4);
    expect(Math.abs(boxes.identity.middle - boxes.interrupt.middle)).toBeLessThanOrEqual(4);
    expect(boxes.raw.x).toBeGreaterThanOrEqual(boxes.identity.right - 1);
    expect(boxes.title.width).toBeGreaterThan(16);
    expect(boxes.title.right).toBeLessThanOrEqual(boxes.raw.x);
    if (width === 390) {
      expect(boxes.header).toBeLessThanOrEqual(64);
      const more = page.locator("#conversation-more");
      await more.click();
      await expect(page.locator("#conversation-commands")).toBeVisible();
      await expect(page.locator("#conversation-show-work")).toBeVisible();
      await expect(page.locator("#conversation-fleet")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(more).toBeFocused();
    } else {
      await expect(page.getByRole("button", { name: "Show work", exact: true })).toBeVisible();
      await expect(page.getByRole("button", { name: "Supervisor commands", exact: true })).toBeVisible();
    }
    await expect(page.locator(".conversation-title-line > #conversation-connection.status-chip")).toBeVisible();
    await expect(page.locator("#conversation-raw-output")).toBeVisible();
    await expect(page.locator("#conversation-interrupt")).toBeVisible();
  }
});
