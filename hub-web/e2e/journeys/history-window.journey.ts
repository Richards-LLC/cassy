import { test, expect, journeyPart } from "./journey";
import { ATLAS, PELICAN } from "./world";
import { JOURNEY_NOW } from "./clock";
import type { Locator, Page } from "@playwright/test";

const historyStamp = (offset: number) => new Date(JOURNEY_NOW + offset).toISOString();

for (const [name, viewport, colorScheme] of [
  ["desktop", { width: 1280, height: 800 }, "light"],
  ["phone", { width: 390, height: 844 }, "dark"],
] as const) {
  test(`HUB-J23 read older turns by scrolling (${name})`, name === "phone" ? journeyPart : {}, async ({ page, journey }) => {
    await page.setViewportSize(viewport); await page.emulateMedia({ colorScheme });
    const turns = (first: number, last: number) => Array.from({ length: last - first + 1 }, (_, index) => {
      const id = first + index;
      return { notification_id: id, reply_to: null, message: `Turn ${id}: release evidence stays in conversation order.`, summary: "", device_id: "journey-device", kind: "answer", at: historyStamp((id - 150) * 6 * 60_000) };
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


test("HUB-J23 recover unanswered history without hiding missing answers (cas-3590)", journeyPart, async ({ page, journey }) => {
  await page.clock.setFixedTime(JOURNEY_NOW);
  const reply = (id: number) => ({ notification_id: id, reply_to: null, message: `Recovered answer ${id}`, summary: "", device_id: "journey-device", kind: "answer", at: historyStamp((id - 101) * 60_000) });
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], history: { [PELICAN]: [
    { messages: [], replies: Array.from({ length: 50 }, (_, index) => reply(51 + index)), has_earlier: true, next_before: 51 },
    { messages: [], replies: [reply(50)], has_earlier: false },
  ] } });
  hub.pausedHistory.add(PELICAN);
  const failure = page.getByRole("alert").filter({ has: page.getByRole("button", { name: "Retry history" }) });
  await journey.stage("An unanswered reopen names the gap and offers recovery", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).and(page.locator(".conversation-row")).click();
    await expect.poll(() => hub.historyRequests.length).toBe(1);
    await page.clock.fastForward(15_000);
    await expectSettledHistoryFailure(page, failure);
    hub.pausedHistory.delete(PELICAN);
    await failure.getByRole("button").click();
    await expect(page.getByRole("log").locator('[data-key="reply:100"]')).toBeVisible();
    await expect(failure).toBeHidden();
  });
  await journey.stage("An unanswered older page can retry its original cursor", async () => {
    // cas-cab9/af35: keep real unanswered supervisor activity in the thread
    // while the history request fails; a quiet fixture misses the late pin.
    const pending = hub.nextSend();
    await page.getByRole("textbox", { name: "Your message" }).fill("Read the missing history while you work");
    await page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }).click();
    expect((await pending).text).toBe("Read the missing history while you work");
    hub.deliverLatest(PELICAN);
    hub.pausedHistory.add(PELICAN);
    // Reconnect A requests the newest page while the cached thread stays readable.
    hub.drop(PELICAN);
    await expect.poll(() => hub.historyRequests.length).toBe(3);
    const newest = hub.historyRequests.at(-1)!;
    expect(newest.before).toBeUndefined();
    await page.getByRole("button", { name: "Load earlier", exact: true }).click();
    await expect.poll(() => hub.historyRequests.length).toBe(4);
    hub.send(PELICAN, { Output: { pane_id: "supervisor", data: [46] } });
    await expect(page.getByRole("log").locator(".working")).toHaveCount(1);
    await page.clock.fastForward(15_000);
    await expectSettledHistoryFailure(page, failure);
    // A's late page hydrates, but B still owes the older answer (cas-bf2f).
    hub.send(PELICAN, { ConversationHistory: { request_id: newest.request_id, messages: [], replies: [reply(100)], has_earlier: true, next_before: 51 } });
    await expect(failure).toBeVisible();
    await expect(page.getByRole("log").locator('[data-key="reply:50"]')).toHaveCount(0);
    hub.pausedHistory.delete(PELICAN);
    await failure.getByRole("button").click();
    await expect(page.getByRole("log").locator('[data-key="reply:50"]')).toBeVisible();
    expect(hub.historyRequests.at(-1)).toMatchObject({ before: 51 });
    await expect(failure).toBeHidden();
    await expect(page.getByRole("log").locator('[data-key="reply:51"]')).toHaveCount(1);
  });
});

async function expectSettledHistoryFailure(page: Page, failure: Locator): Promise<void> {
  await expect(failure).toBeVisible();
  const settled = await page.waitForFunction(state => {
    const thread = document.querySelector<HTMLElement>(".conversation-reading.thread");
    if (!thread) return false;
    state.stable = thread.scrollTop === state.top ? state.stable + 1 : 0;
    state.top = thread.scrollTop;
    return state.stable >= 2;
  }, { top: -1, stable: 0 }, { polling: "raf" });
  await settled.dispose();
  await expect(failure).toBeInViewport();
}
