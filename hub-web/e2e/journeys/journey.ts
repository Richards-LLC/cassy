// Journey fixture: one test per catalog journey (docs/qa/journeys.md), one
// stage per test.step, and a receipt directory per journey:
//   <receipts>/<ID>/journey.webm      screencast, one chapter per stage
//   <receipts>/<ID>/NN-<stage>.png    the screen at the end of each stage
//   <receipts>/<ID>/final.aria.yml    aria snapshot of the goal state
//   <receipts>/<ID>/result.json       id, title, status, per-stage timings
// scripts/journey-eval.sh copies each run's trace.zip next to these.
import { test as base, expect, type Page } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { HubDouble, type DoubleOptions } from "./hub-double";

export { expect };

export const RECEIPTS = resolve(process.env.JOURNEY_RECEIPTS ?? fileURLToPath(new URL("../.results/journeys", import.meta.url)));

type Stage = { title: string; ms: number; screenshot: string };

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
    await page.screencast.start({ path: join(dir, "journey.webm"), size: viewport });
    await page.screencast.showActions({ position: "top-right", duration: 300 });
    let double: HubDouble | undefined;

    const journey: Journey = {
      id,
      async stage(title, body) {
        await test.step(title, async () => {
          await page.screencast.showChapter(title, { duration: 800 });
          const started = Date.now();
          await body();
          const ms = Date.now() - started;
          const screenshot = `${String(stages.length + 1).padStart(2, "0")}-${slug(title)}.png`;
          await settle(page);
          await page.screenshot({ path: join(dir, screenshot) });
          stages.push({ title, ms, screenshot });
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
      output_dir: testInfo.outputDir,
    }, null, 2) + "\n");
    expect(errors, "the page threw while the journey ran").toEqual([]);
  },
});

/** Two animation frames: let the UI paint before a screenshot. */
async function settle(page: Page): Promise<void> {
  await page.evaluate(() => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(ok))));
}
