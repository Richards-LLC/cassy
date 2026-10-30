import { test, expect, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { settle, startJourneyRecording } from "./journeys/journey";
import { JOURNEY_NOW } from "./journeys/clock";

test("journey video bounds encoder work while retaining full-size frames", async ({ page }, testInfo) => {
  const path = testInfo.outputPath("receipt.webm");
  const size = page.viewportSize()!;
  await page.goto("/");
  await startJourneyRecording(page, path, size);
  await page.setContent("<h1>Full-size receipt</h1>");
  await settle(page);
  await page.screencast.stop();

  // pin: Matroska DefaultDuration (0x23E383) declares nanoseconds per encoded
  // frame. Inspect the emitted artifact, not just the options sent to Playwright.
  const video = readFileSync(path);
  const offset = video.indexOf(Buffer.from([0x23, 0xe3, 0x83]));
  expect(offset, "the video declares its frame cadence").toBeGreaterThan(0);
  const length = video[offset + 3]! & 0x7f;
  const frameDuration = video.readUIntBE(offset + 4, length);
  expect(frameDuration, "full-size receipts encode at most ten frames per second").toBeGreaterThanOrEqual(100_000_000);
  await page.route("**/receipt.webm", route => route.fulfill({ contentType: "video/webm", body: video }));
  await page.setContent('<video src="/receipt.webm" preload="metadata"></video>');
  await expect.poll(() => page.evaluate(() => {
    const video = document.querySelector("video")!;
    return { width: video.videoWidth, height: video.videoHeight };
  })).toEqual(size);
});

// Observe browser work without replacing its native execution or scheduling.
function observeEvaluations(page: Page) {
  let pending = 0;
  let notifyEvaluation!: () => void;
  const evaluationStarted = new Promise<void>((resolve) => { notifyEvaluation = resolve; });
  function tracked<T>(work: Promise<T>): Promise<T> {
    pending++;
    return work.finally(() => pending--);
  }
  const observed = new Proxy(page, {
    get(target, key) {
      if (key === "evaluate" || key === "evaluateHandle" || key === "waitForFunction") return (...args: unknown[]) => {
        const evaluation = tracked(Reflect.apply(target[key], target, args));
        notifyEvaluation();
        return evaluation;
      };
      const member = Reflect.get(target, key, target);
      return typeof member === "function" ? member.bind(target) : member;
    },
  });
  return { page: observed, pending: () => pending, evaluationStarted };
}

test("frozen screenshot settle drains evaluations before page teardown", async ({ page }) => {
  await page.clock.install({ time: JOURNEY_NOW });
  await page.clock.pauseAt(JOURNEY_NOW + 60_000);
  await page.setContent("<h1>Held frame</h1>");
  // Paint settling must not depend on the app's held animation callbacks.
  await page.evaluate(() => {
    window.requestAnimationFrame = () => { throw new Error("page animation callbacks are held"); };
  });
  const instant = await page.evaluate(() => performance.now());
  const observed = observeEvaluations(page);

  await settle(observed.page);
  expect(observed.pending(), "settle leaves no native evaluation pending").toBe(0);
  expect(await page.evaluate(() => performance.now()), "screenshot settling does not advance protocol time").toBe(instant);
  await expect(page.getByRole("heading", { name: "Held frame" })).toBeVisible();
  expect((await page.screenshot()).subarray(0, 8)).toEqual(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]));
  await page.close();
  expect(observed.pending()).toBe(0);
});

test("screenshot settle has a deadline when the renderer is suspended", async ({ page }) => {
  await page.setContent("<h1>Product assertions passed</h1>");
  await expect(page.getByRole("heading")).toHaveText("Product assertions passed");
  const session = await page.context().newCDPSession(page);
  // Pause JS execution, including native paint polling and cleanup. Pausing
  // virtual time alone can still deliver already queued native frames.
  await session.send("Debugger.enable");
  const paused = new Promise<void>((resolve) => { session.once("Debugger.paused", () => resolve()); });
  await session.send("Debugger.pause");
  await paused;
  try {
    await expect(settle(page)).rejects.toThrow(/Timeout .* exceeded/);
  } finally {
    await session.send("Debugger.disable");
    await session.detach();
  }
});

test("running screenshot settle waits for both animation frames", async ({ page }) => {
  await page.clock.install({ time: JOURNEY_NOW });
  await page.setContent("<h1>Before paint</h1>");
  await page.evaluate(() => {
    requestAnimationFrame(() => requestAnimationFrame(() => {
      document.querySelector("h1")!.textContent = "Painted twice";
    }));
  });
  const observed = observeEvaluations(page);
  await settle(observed.page);
  expect(observed.pending()).toBe(0);
  expect(await page.getByRole("heading").textContent()).toBe("Painted twice");
});

test("page closure during frozen settling rejects and drains the evaluation", async ({ page }) => {
  await page.clock.install({ time: JOURNEY_NOW });
  await page.clock.pauseAt(JOURNEY_NOW + 60_000);
  const observed = observeEvaluations(page);
  const settling = settle(observed.page);
  // Observe rejection before triggering closure, even if it wins the race.
  const rejected = expect(settling).rejects.toThrow(/closed/);
  await observed.evaluationStarted;
  await page.close();
  await rejected;
  expect(observed.pending()).toBe(0);
});
