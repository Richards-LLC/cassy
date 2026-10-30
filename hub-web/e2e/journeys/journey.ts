// Journey fixture: one test per catalog journey (docs/qa/journeys.md), one
// stage per test.step, and a receipt directory per journey:
//   <receipts>/<ID>/receipt.webm      screencast, one chapter per stage
//   <receipts>/<ID>/J01.png, J02.png  the screen at the end of each stage
//   <receipts>/<ID>/final.aria.yml    aria snapshot of the goal state (+ .json)
//   <receipts>/<ID>/result.json       id, title, status, per-stage timings
// scripts/journey-eval.sh keeps one passing trace per bundle for qa_evidence,
// keeps failed or incomplete diagnostics, and writes bundle.json (producer
// "journey"). Redundant passing traces are discarded.
import { test as base, expect, type Page } from "@playwright/test";
import { writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { HubDouble, type DoubleOptions } from "./hub-double";
import { JOURNEY_NOW, JOURNEY_TIMEZONE, startJourneyClock, stopJourneyClock } from "./clock";
import { captureMask, captureStageScreenshot, claimReceiptDirectory, receiptPartName } from "./receipt-directory.mjs";

export { expect };

// The default receipt root sits inside the Playwright output directory
// (JOURNEY_OUTPUT or e2e/.results), which Playwright empties at the start of
// every run, so a rerun never finds the previous run's receipt claims.
// journey-eval.sh passes its own fresh JOURNEY_RECEIPTS directory.
export const RECEIPTS = resolve(process.env.JOURNEY_RECEIPTS ?? join(process.env.JOURNEY_OUTPUT ?? fileURLToPath(new URL("../.results", import.meta.url)), "journeys"));

/**
 * Whether this run records the visual receipt: the screencast with its chapter
 * and action overlays, and a screenshot per stage (cas-c4b2). journey-eval.sh
 * (worker and QA evidence) always asks for receipts with JOURNEY_RECEIPTS, and
 * a local run records by default. A CI run that asks for none, the merge
 * queue, skips them: nothing reads them there, and on a 4-vCPU hosted runner
 * the encoder and each overlay's fixed pause (a second per chapter) pushed
 * multi-stage journeys past the test timeout. JOURNEY_RECORD=1|0 overrides.
 * The assertions, result.json, aria snapshots and frame checks always run.
 */
export const RECORD = process.env.JOURNEY_RECORD !== undefined
  ? process.env.JOURNEY_RECORD === "1"
  : process.env.JOURNEY_RECEIPTS !== undefined || !process.env.CI;

const CAPTURE_MASK = captureMask(process.env.JOURNEY_CAPTURE_MASK);
const CAPTURE_HTML_BYTES = process.env.JOURNEY_CAPTURE_MAX_HTML_BYTES === undefined
  ? undefined : Number(process.env.JOURNEY_CAPTURE_MAX_HTML_BYTES);
if (CAPTURE_HTML_BYTES !== undefined && (!Number.isSafeInteger(CAPTURE_HTML_BYTES) || CAPTURE_HTML_BYTES <= 0)) {
  throw new Error("JOURNEY_CAPTURE_MAX_HTML_BYTES must be a positive integer");
}

type Stage = { title: string; slug: string; ms: number; screenshot?: string };

export type Journey = {
  id: string;
  /** Run one user-visible stage: a test.step, a screencast chapter, a timing and a screenshot. */
  stage(title: string, body: () => Promise<void>): Promise<void>;
  /** Install the hub double; seeds paired machines when `paired` is set. */
  hub(options: DoubleOptions): Promise<HubDouble>;
  /** Open Cassy Commander the way a user does: its /commander/ URL. */
  open(): Promise<void>;
};

/**
 * Marks a test as one part of a catalog journey that runs as several tests
 * (cas-1f7e): `test("HUB-J12 …", JOURNEY_PART, async ({ page, journey }) => …)`.
 */
export const JOURNEY_PART = { type: "journey-part", description: "one part of a catalog journey run as several tests" } as const;
export const journeyPart = { annotation: JOURNEY_PART };

function slug(text: string): string {
  return text.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 48);
}

/**
 * The keyboard platform a journey's browser declares (cas-2a33). Commander
 * names the palette chord from navigator.userAgentData.platform, else
 * navigator.platform (applePlatform): "⌘K" on Apple, "Ctrl K" elsewhere.
 * The Desktop Chrome device only overrides the user agent, so what a run saw
 * depended on the host: a Windows UA beside a real "MacIntel" platform, and
 * on some Chromium builds no userAgentData at all, so a macOS host read ⌘K
 * where a Linux CI host read Ctrl K. Every journey now declares one platform,
 * consistently in both properties: Linux by default, and macOS where a test
 * asks for it with test.use({ journeyPlatform: "mac" }).
 */
export type JourneyPlatform = "linux" | "mac";

const PLATFORM_NAMES: Record<JourneyPlatform, { platform: string; uaPlatform: string }> = {
  linux: { platform: "Linux x86_64", uaPlatform: "Linux" },
  mac: { platform: "MacIntel", uaPlatform: "macOS" },
};

export function startJourneyRecording(page: Page, path: string, size: { width: number; height: number }) {
  // Full-size VP9 at Playwright's default 25 fps can queue more encoding than
  // the host can drain before context teardown. Ten fps keeps three frames of
  // each 300 ms action annotation; stage screenshots retain the exact pixels.
  return page.screencast.start({ path, size, fps: 10 });
}

export const test = base.extend<{ journey: Journey; journeyPlatform: JourneyPlatform }>({
  journeyPlatform: ["linux", { option: true }],
  page: async ({ page, journeyPlatform }, use) => {
    await page.addInitScript(({ platform, uaPlatform }) => {
      const real = (navigator as unknown as { userAgentData?: object }).userAgentData;
      const data = real
        ? new Proxy(real, { get: (target, key) => key === "platform" ? uaPlatform : Reflect.get(target, key, target) })
        : { platform: uaPlatform, mobile: false, brands: [] };
      // On the navigator itself, so it shadows whatever the host's Chromium
      // puts on Navigator.prototype (or leaves off it).
      Object.defineProperty(navigator, "platform", { get: () => platform, configurable: true });
      Object.defineProperty(navigator, "userAgentData", { get: () => data, configurable: true });
    }, PLATFORM_NAMES[journeyPlatform]);
    // Install before navigation; time flows normally from a known instant.
    // Freezing Date would stop the app aging receipts and heartbeat deadlines.
    await page.clock.install({ time: startJourneyClock() });
    try {
      await use(page);
    } finally {
      stopJourneyClock();
    }
  },
  journey: async ({ page }, use, testInfo) => {
    const id = /^([A-Z]+-J[0-9]+)\b/.exec(testInfo.title)?.[1];
    if (!id) throw new Error(`journey test titles must start with a catalog id: "${testInfo.title}"`);
    // cas-1f7e: one catalog journey can run as several tests (HUB-J12's
    // network switches). Each extra test is marked with JOURNEY_PART and
    // writes its receipts under <ID>/parts/<slug>-<identity>/, so no test overwrites
    // another's; journey-bundles.py folds the parts into the <ID> bundle.
    const identity = { project: testInfo.project.name, titlePath: testInfo.titlePath };
    const part = testInfo.annotations.some((annotation) => annotation.type === JOURNEY_PART.type)
      ? receiptPartName(testInfo.title, identity)
      : undefined;
    // Concurrent repeats must keep their captures instead of clearing another
    // attempt's files through claimReceiptDirectory's retry cleanup.
    // CLI --repeat-each does not change project.repeatEach. The first attempt
    // keeps the ordinary bundle path; every later repeat has its own root.
    const root = testInfo.repeatEachIndex > 0 ? join(RECEIPTS, `repeat-${testInfo.repeatEachIndex + 1}`) : RECEIPTS;
    const dir = part ? join(root, id, "parts", part) : join(root, id);
    claimReceiptDirectory(dir, testInfo.title, identity);
    const stages: Stage[] = [];
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const viewport = page.viewportSize() ?? { width: 1280, height: 800 };
    // Full-size frames need trace `screenshots: false` (see playwright.config.ts).
    if (RECORD) {
      await startJourneyRecording(page, join(dir, "receipt.webm"), viewport);
      await page.screencast.showActions({ position: "top-right", duration: 300 });
    }
    let double: HubDouble | undefined;
    const frames = await watchConversationFrames(page);

    const journey: Journey = {
      id,
      async stage(title, body) {
        await test.step(title, async () => {
          if (RECORD) await page.screencast.showChapter(title, { duration: 1000 });
          const started = performance.now();
          await body();
          const ms = performance.now() - started;
          if (!RECORD) {
            stages.push({ title, slug: slug(title), ms });
            return;
          }
          const screenshot = `J${String(stages.length + 1).padStart(2, "0")}.png`;
          await settle(page);
          if (CAPTURE_MASK === undefined && CAPTURE_HTML_BYTES === undefined) {
            await page.screenshot({ path: join(dir, screenshot) });
          } else {
            await captureStageScreenshot(page, join(dir, screenshot), CAPTURE_MASK, CAPTURE_HTML_BYTES);
          }
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
    if (RECORD) await test.step("Finalize journey recording", () => page.screencast.stop().catch(() => undefined));
    const status = testInfo.status === "passed" && errors.length === 0 ? "PASS" : "FAIL";
    writeFileSync(join(dir, "result.json"), JSON.stringify({
      id,
      title: testInfo.title.replace(/^[A-Z]+-J[0-9]+\s*/, ""),
      status,
      label: "real-bundle, protocol-double",
      project: testInfo.project.name,
      title_path: testInfo.titlePath,
      viewport,
      ...(CAPTURE_MASK === undefined ? {} : {
        masked: CAPTURE_MASK,
        color_scheme: await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light"),
      }),
      clock: { now: new Date(JOURNEY_NOW).toISOString(), timezone: JOURNEY_TIMEZONE },
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
export async function expectWholeFocusRing(field: import("@playwright/test").Locator, options: { vertical?: boolean } = {}): Promise<void> {
  await expect(field).toBeFocused();
  const ring = await field.evaluate((el, vertical) => {
    const style = getComputedStyle(el);
    const extent = parseFloat(style.outlineWidth) + parseFloat(style.outlineOffset);
    const box = el.getBoundingClientRect();
    let clip = el.parentElement;
    while (clip && !/auto|scroll|hidden|clip/.test(getComputedStyle(clip).overflowX + (vertical ? getComputedStyle(clip).overflowY : ""))) clip = clip.parentElement;
    if (!clip) return { outline: style.outlineStyle, fits: true };
    const bounds = clip.getBoundingClientRect();
    const clipStyle = getComputedStyle(clip);
    const left = bounds.left + parseFloat(clipStyle.borderLeftWidth);
    const right = bounds.right - parseFloat(clipStyle.borderRightWidth);
    const top = bounds.top + parseFloat(clipStyle.borderTopWidth);
    const bottom = bounds.bottom - parseFloat(clipStyle.borderBottomWidth);
    const across = box.left - extent >= left - 0.5 && box.right + extent <= right + 0.5;
    // A control that fills a clipping heading (a clipped title) loses
    // its ring top and bottom as well (cas-cf10 QA F01).
    const down = !vertical || (box.top - extent >= top - 0.5 && box.bottom + extent <= bottom + 0.5);
    return { outline: style.outlineStyle, fits: across && down };
  }, options.vertical === true);
  expect(ring, "the focused field shows its whole focus ring").toEqual({ outline: "solid", fits: true });
}

/**
 * Hold the page clock a moment ahead of the page's own now (cas-c4b2). Time
 * then moves only through page.clock.runFor/fastForward, so a step that
 * stands for a measured delay is counted on the page clock alone, never
 * stretched by a loaded host. Resume with page.clock.resume().
 *
 * The page's now is its installed clock's origin plus the ticks run since
 * (journeys never set the system time). It is read just before the hold, and
 * read again if a slow round trip let the clock pass it.
 */
export async function holdPageClock(page: Page, ahead = 250): Promise<void> {
  for (let attempt = 1; ; attempt += 1) {
    const now = await page.evaluate(() => performance.timeOrigin + performance.now());
    try {
      await page.clock.pauseAt(now + ahead);
      return;
    } catch (error) {
      if (attempt === 3 || !String(error).includes("Cannot fast-forward to the past")) throw error;
    }
  }
}

/** Two animation frames: let the UI paint before a screenshot. */
export async function settle(page: Page): Promise<void> {
  // Playwright polls native browser frames even when ProtocolClock holds the
  // app's RAF callbacks. The first predicate runs immediately, then two paint
  // opportunities follow. Keep the old 250ms budget on the native operation:
  // a host Promise.race did not bound the finish/drain evaluations in finally.
  // Missing native frames now fail capture instead of leaving cleanup waiting
  // on the same renderer that could not finish painting (cas-fc03).
  const painted = await page.waitForFunction((state) => ++state.frames === 3,
    { frames: 0 }, { polling: "raf", timeout: 250 });
  await painted.dispose();
}

/**
 * Every animation frame that can show a change, on every page load: a
 * conversation must never show the terminal canvas (a near-black frame in the
 * light theme) and never sit on a bare panel (cas-04ee, journey evaluation F9/F13). A bare panel is allowed
 * for a moment while one view replaces another, not for 250 ms.
 */
async function watchConversationFrames(page: Page): Promise<string[]> {
  const defects: string[] = [];
  await page.exposeFunction("__journeyFrameDefect", (defect: string) => { if (!defects.includes(defect)) defects.push(defect); });
  await page.addInitScript(() => {
    const report = (window as unknown as { __journeyFrameDefect: (defect: string) => void }).__journeyFrameDefect;
    let bareSince: number | undefined;
    // Frames are watched only while something can change what they show: the
    // DOM, the viewport, a transition or an animation. A quiet page queues no
    // frame callback. A held page clock (ProtocolClock) fires every queued one,
    // 125 per 2 s tick, and an always-on loop made the clock-heavy HUB-J12
    // parts 4x slower and stall under load (cas-1f7e). A bare panel keeps being
    // timed frame by frame until it fills or goes.
    let changed = true;
    let queued = false;
    const watch = () => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(tick);
    };
    const touched = () => { changed = true; watch(); };
    const tick = (now: number) => {
      queued = false;
      if (changed) {
        changed = false;
        const slot = document.querySelector<HTMLElement>(".conversation-pane-slot");
        if (slot) {
          for (const canvas of slot.querySelectorAll("canvas")) {
            const box = canvas.getBoundingClientRect();
            if (getComputedStyle(canvas).visibility === "visible" && box.width > 0 && box.height > 0) report("terminal canvas visible in the conversation");
          }
          if (!slot.innerText.trim()) bareSince ??= now;
          else bareSince = undefined;
        } else bareSince = undefined;
      }
      if (bareSince === undefined) return;
      if (now - bareSince > 250) report("conversation panel bare for over 250 ms");
      watch();
    };
    new MutationObserver(touched).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    for (const event of ["resize", "transitionrun", "transitionend", "animationstart", "animationend"]) addEventListener(event, touched, true);
    watch();
  });
  return defects;
}
