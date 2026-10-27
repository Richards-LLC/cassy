import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { journeyStamp } from "./clock";

for (const [name, viewport, colorScheme] of [
  ["desktop", { width: 1280, height: 800 }, "light"],
  ["phone", { width: 390, height: 844 }, "dark"],
] as const) {
  test(`HUB-J23 read older turns by scrolling (${name})`, name === "phone" ? journeyPart : {}, async ({ page, journey }) => {
    await page.setViewportSize(viewport); await page.emulateMedia({ colorScheme });
    const turns = (first: number, last: number) => Array.from({ length: last - first + 1 }, (_, index) => {
      const id = first + index;
      return { notification_id: id, reply_to: null, message: `Turn ${id}: release evidence stays in conversation order.`, summary: "", device_id: "journey-device", kind: "answer", at: journeyStamp((id - 150) * 1000) };
    });
    const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], history: { [PELICAN]: [
      { has_earlier: true, next_before: 101, messages: [], replies: turns(101, 150) },
      { has_earlier: true, next_before: 51, messages: [], replies: turns(51, 100) },
      { has_earlier: false, messages: [], replies: turns(1, 50) },
    ] } });
    const thread = page.locator(".conversation-reading.thread");
    const log = page.getByRole("log");
    await journey.stage("Open directly at the latest exchange", async () => {
      await journey.open();
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
      await expect(log.locator('[data-key="reply:150"]')).toBeInViewport();
      expect(hub.historyRequests).toHaveLength(1);
      expect(hub.historyRequests[0]).toMatchObject({ limit: 50 });
      await expect(page.getByRole("button", { name: "Jump to latest" })).toBeHidden();
    });
    await journey.stage("Scroll up to request one older page", async () => {
      // Stop above the tail, then use an actual reading gesture near the top.
      await thread.evaluate(element => { element.scrollTop = 300; });
      await thread.hover(); await page.mouse.wheel(0, -250);
      await expect.poll(() => hub.historyRequests.length).toBe(2);
      expect(hub.historyRequests.at(-1)).toMatchObject({ before: 101 });
      await expect.poll(() => log.locator(".conversation-turn").count()).toBeLessThanOrEqual(80);
      await expect(log.locator('[data-key="reply:150"]')).not.toBeAttached();
    });
    await journey.stage("Return to the latest exchange without fetching history", async () => {
      await page.getByRole("button", { name: "Jump to latest" }).click();
      await expect(log.locator('[data-key="reply:150"]')).toBeInViewport();
      expect(hub.historyRequests).toHaveLength(2);
      const ids = await log.locator(".conversation-turn").evaluateAll(nodes => nodes.map(node => Number((node as HTMLElement).dataset.key!.split(":")[1])));
      expect(ids).toEqual([...ids].sort((a, b) => a - b));
      expect(new Set(ids).size).toBe(ids.length);
    });
  });
}
