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
      return { notification_id: id, reply_to: null, message: `Turn ${id}: release evidence stays in conversation order.`, summary: "", device_id: "journey-device", kind: "answer", at: journeyStamp((id - 150) * 6 * 60_000) };
    });
    const deepAsk = { ...turns(1, 1)[0]!, kind: "ask", message: "Approve the release plan from earlier history?", options: ["Approve"] };
    const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], history: { [PELICAN]: [
      { has_earlier: true, next_before: 101, messages: [], replies: turns(101, 150), open_attention: [deepAsk] },
      { has_earlier: true, next_before: 51, messages: [], replies: turns(51, 100) },
      { has_earlier: false, messages: [], replies: turns(2, 50) },
    ] } });
    const thread = page.locator(".conversation-reading.thread");
    const log = page.getByRole("log");
    await journey.stage("Open directly at the latest exchange", async () => {
      await journey.open();
      await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
      await expect(log.locator('[data-key="reply:150"]')).toBeInViewport();
      expect(hub.historyRequests).toHaveLength(1);
      expect(hub.historyRequests[0]).toMatchObject({ limit: 50 });
      await expect(page.getByRole("button", { name: "Jump to latest" })).toBeHidden();
    });
    await journey.stage("Read cached older windows without getting stuck", async () => {
      const first = () => log.locator(".conversation-turn").first().getAttribute("data-key");
      for (let clicks = 0; clicks < 8 && hub.historyRequests.length === 1; clicks++) {
        const before = await first();
        await page.getByRole("button", { name: "Load earlier", exact: true }).click();
        await expect.poll(async () => hub.historyRequests.length > 1 || await first() !== before,
          { message: "Load earlier reveals another cached window or requests the next page" }).toBe(true);
      }
      expect(hub.historyRequests).toHaveLength(2);
      await page.getByRole("button", { name: "Jump to latest" }).click();
      await expect(log.locator('[data-key="reply:150"]')).toBeInViewport();
    });
    await journey.stage("Scroll up to request one older page", async () => {
      // Stop above the tail, then use an actual reading gesture near the top.
      await thread.evaluate(element => { element.scrollTop = 300; });
      await thread.hover(); await page.mouse.wheel(0, -250);
      await expect.poll(() => hub.historyRequests.length).toBe(3);
      expect(hub.historyRequests.at(-1)).toMatchObject({ before: 51 });
      await expect.poll(() => log.locator(".conversation-turn").count()).toBeLessThanOrEqual(80);
      await expect(log.locator('[data-key="reply:150"]')).not.toBeInViewport();
    });
    await journey.stage("Return to the latest exchange without fetching history", async () => {
      await page.getByRole("button", { name: "Jump to latest" }).click();
      await expect(log.locator('[data-key="reply:150"]')).toBeInViewport();
      expect(hub.historyRequests).toHaveLength(3);
      const ids = await log.locator(".conversation-turn").evaluateAll(nodes => nodes.map(node => Number((node as HTMLElement).dataset.key!.split(":")[1])));
      expect(ids).toEqual([...ids].sort((a, b) => a - b));
      expect(new Set(ids).size).toBe(ids.length);
    });
    await journey.stage("Answer a decision from before the loaded tail", async () => {
      await page.getByRole("button", { name: "Waiting on you: Approve the release plan from earlier history?" }).click();
      const question = log.getByRole("group", { name: `Question from ${PELICAN}` }).filter({ hasText: deepAsk.message });
      await expect(question).toBeInViewport();
      const sent = hub.nextSend();
      await question.getByRole("button", { name: "Approve", exact: true }).click();
      expect(await sent).toMatchObject({ text: "Approve", in_reply_to: 1 });
      expect(hub.historyRequests).toHaveLength(3);
    });
  });
}
