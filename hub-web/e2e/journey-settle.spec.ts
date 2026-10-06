import { test, expect, type Page, type JSHandle } from "@playwright/test";
import { settle } from "./journeys/journey";
import { JOURNEY_NOW } from "./journeys/clock";

// Observe real native evaluations, including evaluations on returned handles.
// A host timeout is insufficient if an evaluation still owns a frozen RAF.
function observeEvaluations(page: Page) {
  let pending = 0;
  function tracked<T>(work: Promise<T>): Promise<T> {
    pending++;
    return work.finally(() => pending--);
  }
  function handle(value: JSHandle): JSHandle {
    return new Proxy(value, {
      get(target, key) {
        if (key === "evaluate") return (...args: unknown[]) => tracked(Reflect.apply(target.evaluate, target, args));
        const member = Reflect.get(target, key, target);
        return typeof member === "function" ? member.bind(target) : member;
      },
    });
  }
  const observed = new Proxy(page, {
    get(target, key) {
      if (key === "evaluate") return (...args: unknown[]) => tracked(Reflect.apply(target.evaluate, target, args));
      if (key === "evaluateHandle") return (...args: unknown[]) => tracked(Reflect.apply(target.evaluateHandle, target, args)).then(handle);
      const member = Reflect.get(target, key, target);
      return typeof member === "function" ? member.bind(target) : member;
    },
  });
  return { page: observed, pending: () => pending };
}

test("frozen screenshot settle drains evaluations before page teardown", async ({ page }) => {
  await page.clock.install({ time: JOURNEY_NOW });
  await page.clock.pauseAt(JOURNEY_NOW + 60_000);
  await page.setContent("<h1>Held frame</h1>");
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

test("running screenshot settle waits for both animation frames", async ({ page }) => {
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
