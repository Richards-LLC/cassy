// Journey fixture: one test per catalog journey (docs/qa/journeys.md), one
// stage per test.step, and a receipt directory per journey:
//   <receipts>/<ID>/receipt.webm      screencast, one chapter per stage
//   <receipts>/<ID>/J01.png, J02.png  the screen at the end of each stage
//   <receipts>/<ID>/final.aria.yml    aria snapshot of the goal state (+ .json)
//   <receipts>/<ID>/result.json       id, title, status, per-stage timings
// scripts/journey-eval.sh adds trace.zip, trace-actions.txt and bundle.json,
// the cas-qa-craft evidence-bundle shape with producer "journey".
import { test as base, expect, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { HubDouble, type DoubleOptions } from "./hub-double";

export { expect };

export const RECEIPTS = resolve(process.env.JOURNEY_RECEIPTS ?? fileURLToPath(new URL("../.results/journeys", import.meta.url)));

type Stage = { title: string; slug: string; ms: number; screenshot: string };

export type Journey = {
  id: string;
  /** Run one user-visible stage: a test.step, a screencast chapter, a timing and a screenshot. */
  stage(title: string, body: () => Promise<void>): Promise<void>;
  /** Install the hub double; seeds paired machines when `paired` is set. */
  hub(options: DoubleOptions): Promise<HubDouble>;
  /** Open Cassy Commander the way a user does: its /commander/ URL. */
  open(): Promise<void>;
};

function slug(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 48);
}

export const test = base.extend<{ journey: Journey }>({
  journey: async ({ page }, use, testInfo) => {
    const id = /^([A-Z]+-J[0-9]+)\b/.exec(testInfo.title)?.[1];
    if (!id) throw new Error(`journey test titles must start with a catalog id: "${testInfo.title}"`);
    const dir = join(RECEIPTS, id);
    mkdirSync(dir, { recursive: true });
    const stages: Stage[] = [];
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const viewport = page.viewportSize() ?? { width: 1280, height: 800 };
    // Full-size frames need trace `screenshots: false` (see playwright.config.ts).
    await page.screencast.start({ path: join(dir, "receipt.webm"), size: viewport });
    await page.screencast.showActions({ position: "top-right", duration: 300 });
    let double: HubDouble | undefined;
    const frames = await watchConversationFrames(page);

    const journey: Journey = {
      id,
      async stage(title, body) {
        await test.step(title, async () => {
          await page.screencast.showChapter(title, { duration: 1000 });
          const started = Date.now();
          await body();
          const ms = Date.now() - started;
          const screenshot = `J${String(stages.length + 1).padStart(2, "0")}.png`;
          await settle(page);
          await page.screenshot({ path: join(dir, screenshot) });
          stages.push({ title, slug: slug(title), ms, screenshot });
        }, { subtitle: id });
      },
      async hub(options) {
        double = new HubDouble(page, options);
        await double.install();
        if (options.paired?.length) {
          await page.goto("./");
          await double.seedPaired();
        }
        return double;
      },
      async open() {
        await page.goto("./");
      },
    };

    await use(journey);

    let aria = "";
    try { aria = await page.locator("body").ariaSnapshot(); } catch (error) { aria = `# aria snapshot failed: ${String(error)}`; }
    writeFileSync(join(dir, "final.aria.yml"), aria + "\n");
    let ariaJson: unknown = null;
    try { ariaJson = await page.locator("body").ariaSnapshotJSON(); } catch (error) { ariaJson = { error: String(error) }; }
    writeFileSync(join(dir, "final.aria.json"), JSON.stringify(ariaJson, null, 2) + "\n");
    await page.screencast.stop().catch(() => undefined);
    const status = testInfo.status === "passed" && errors.length === 0 ? "PASS" : "FAIL";
    writeFileSync(join(dir, "result.json"), JSON.stringify({
      id,
      title: testInfo.title.replace(/^[A-Z]+-J[0-9]+\s*/, ""),
      status,
      label: "real-bundle, protocol-double",
      project: testInfo.project.name,
      viewport,
      stages,
      page_errors: errors,
      frame_defects: frames,
      output_dir: testInfo.outputDir,
    }, null, 2) + "\n");
    expect(errors, "the page threw while the journey ran").toEqual([]);
    expect(frames, "a conversation showed the terminal frame or a bare panel").toEqual([]);
  },
});

/**
 * The focused field draws the standard focus ring whole: its outline (width
 * plus offset) fits inside the nearest clipping ancestor, so no side is cut
 * off into stray bars (cas-b2e4 F03).
 */
export async function expectWholeFocusRing(field: import("@playwright/test").Locator): Promise<void> {
  await expect(field).toBeFocused();
  const ring = await field.evaluate((el) => {
    const style = getComputedStyle(el);
    const extent = parseFloat(style.outlineWidth) + parseFloat(style.outlineOffset);
    const box = el.getBoundingClientRect();
    let clip = el.parentElement;
    while (clip && !/auto|scroll|hidden|clip/.test(getComputedStyle(clip).overflowX)) clip = clip.parentElement;
    if (!clip) return { outline: style.outlineStyle, fits: true };
    const bounds = clip.getBoundingClientRect();
    const clipStyle = getComputedStyle(clip);
    const left = bounds.left + parseFloat(clipStyle.borderLeftWidth);
    const right = bounds.right - parseFloat(clipStyle.borderRightWidth);
    return { outline: style.outlineStyle, fits: box.left - extent >= left - 0.5 && box.right + extent <= right + 0.5 };
  });
  expect(ring, "the focused field shows its whole focus ring").toEqual({ outline: "solid", fits: true });
}

/** Two animation frames: let the UI paint before a screenshot. */
async function settle(page: Page): Promise<void> {
  await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
}

/**
 * Every animation frame, on every page load: a conversation must never show
 * the terminal canvas (a near-black frame in the light theme) and never sit on
 * a bare panel (cas-04ee, journey evaluation F9/F13). A bare panel is allowed
 * for a moment while one view replaces another, not for 250 ms.
 */
async function watchConversationFrames(page: Page): Promise<string[]> {
  const defects: string[] = [];
  await page.exposeFunction("__journeyFrameDefect", (defect: string) => { if (!defects.includes(defect)) defects.push(defect); });
  await page.addInitScript(() => {
    const report = (window as unknown as { __journeyFrameDefect: (defect: string) => void }).__journeyFrameDefect;
    let bareSince: number | undefined;
    const tick = (now: number) => {
      const slot = document.querySelector<HTMLElement>(".conversation-pane-slot");
      if (slot) {
        for (const canvas of slot.querySelectorAll("canvas")) {
          const box = canvas.getBoundingClientRect();
          if (getComputedStyle(canvas).visibility === "visible" && box.width > 0 && box.height > 0) report("terminal canvas visible in the conversation");
        }
        if (!slot.innerText.trim()) {
          bareSince ??= now;
          if (now - bareSince > 250) report("conversation panel bare for over 250 ms");
        } else bareSince = undefined;
      } else bareSince = undefined;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  return defects;
}
