import { activate, phoneLayout, phoneAsk, showConversationList, openConversation } from "./responsive-goals";
import { test, expect, journeyPart } from "./journey";
import { journeyNow, journeyStamp } from "./clock";
import type { Page } from "@playwright/test";
import { ATLAS, STUDIO, PELICAN, OTTER } from "./world";

/** The operator's latest message group: its label time, and the day separator above it. */
async function lastSend(page: Page): Promise<{ label: string; day: string }> {
  return page.evaluate(() => {
    const groups = [...document.querySelectorAll<HTMLElement>('.msgs [role="group"][aria-label^="You, "]')];
    const group = groups.at(-1)!;
    let day = "";
    for (let node: Element | null = group; node; node = node.previousElementSibling) if (node.classList.contains("day")) { day = node.textContent ?? ""; break; }
    return { label: group.getAttribute("aria-label") ?? "", day };
  });
}
/** The thread as painted, top to bottom: day and session lines by text, message groups by their spoken label. */
async function threadOrder(page: Page): Promise<string[]> {
  return page.locator(".msgs > *").evaluateAll((nodes) => nodes.filter((node) => node.matches(".day, .session-divider, [role=group]")).map((node) => node.getAttribute("role") === "group" ? node.getAttribute("aria-label") ?? "" : node.textContent ?? ""));
}
/** A finger swiping `element` sideways by `dx` (touch pointer events, as a phone sends them). */
async function swipeAway(page: Page, selector: string, dx: number): Promise<void> {
  await page.locator(selector).first().evaluate((element, dx) => {
    const box = element.getBoundingClientRect();
    const y = box.top + Math.min(24, box.height / 2);
    const x0 = box.left + box.width / 2;
    const fire = (type: string, x: number) => element.dispatchEvent(new PointerEvent(type, { bubbles: true, cancelable: true, pointerType: "touch", pointerId: 7, isPrimary: true, clientX: x, clientY: y }));
    fire("pointerdown", x0);
    for (let step = 1; step <= 6; step += 1) fire("pointermove", x0 + (dx * step) / 6);
    fire("pointerup", x0 + dx);
  }, dx);
}
function youNow(): string[] {
  // Browser and Node clocks advance independently; tolerate a minute boundary
  // during the send while keeping both anchored to the injected UTC instant.
  return [-60_000, 0, 60_000].map((offset) => {
    const d = new Date(journeyNow() + offset);
    return `You, ${String(d.getUTCHours()).padStart(2, "0")}:${String(d.getUTCMinutes()).padStart(2, "0")}`;
  });
}

test("HUB-J7 answer a question in the thread", async ({ page, journey }) => {
  if (await phoneLayout(page)) { await phoneAsk(page, journey); return; }
  // The machine's clock runs five minutes ahead of this browser's, and its
  // durable history holds an open blocker (cas-ce17).
  const ahead = journeyStamp(300_000);
  // The second machine's clock runs a whole day ahead (cas-ac1f).
  const dayAhead = journeyStamp(86_400_000);
  const hub = await journey.hub({
    machines: [ATLAS, STUDIO],
    paired: ["atlas", "studio"],
    // Everything each machine stamps comes from its own clock.
    clockAheadMs: { [PELICAN]: 300_000, [OTTER]: 86_400_000 },
    history: {
      // cas-16eed: a question the previous supervisor session asked and never
      // had answered here (it was settled in the pane) sits before the blocker.
      [PELICAN]: [{ has_earlier: false, messages: [], replies: [
        { notification_id: 899, reply_to: null, message: "All waves are done; the full suite passes.\n\n- **Ask:** open the PR to main and cut a release?", summary: "", device_id: "journey-device", kind: "ask", attachments: [], session: "patient-pelican-8", at: journeyStamp(-60_000) },
        { notification_id: 900, reply_to: null, message: "The release gate went red; the train is held.", summary: "", device_id: "journey-device", kind: "blocker", attachments: [], at: ahead },
      ] }],
      [OTTER]: [{ has_earlier: false, messages: [], replies: [{ notification_id: 901, reply_to: null, message: "Mac build is queued behind the nightly.", summary: "", device_id: "journey-device", kind: "answer", attachments: [], at: dayAhead }] }],
    },
  });
  const waiting = page.locator('[aria-label="Conversation context"] [data-section="waiting"]');
  const pinned = page.getByRole("region", { name: `Waiting on you: question from ${PELICAN}` });
  let ask = 0;

  await journey.stage("Open the conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true })).toBeVisible();
  });

  await journey.stage("A question from an ended session does not wait", async () => {
    // cas-16eed, cas-55a4: the previous session's release question is
    // history, not a pinned demand and not this session's thread. It sits in
    // the collapsed "Earlier session" section, offers no choices; nothing is
    // pinned and the rail lists only the live blocker.
    await expect(page.getByRole("log")).not.toContainText("open the PR to main and cut a release?");
    const earlier = page.getByRole("region", { name: "Earlier sessions" }).locator("details");
    await expect(earlier.locator("summary")).toContainText("Earlier session patient-pelican-8");
    await expect(earlier).not.toHaveAttribute("open", /.*/);
    await earlier.locator("summary").click();
    await expect(earlier).toContainText("open the PR to main and cut a release?");
    // cas-8d52 (journey F13): the ended session's turn is its own supervisor's.
    await expect(earlier.locator(".earlier-turn.supervisor b")).toHaveText("patient-pelican-8");
    await expect(earlier.getByRole("button")).toHaveCount(0);
    await earlier.locator("summary").click();
    await expect(pinned).toBeHidden();
    await expect(waiting.locator("li")).toHaveCount(1);
    await expect(waiting.locator('.context-jump[data-kind="ask"]')).toHaveCount(0);
  });

  await journey.stage("The supervisor asks a question", async () => {
    ask = hub.supervisorSays(PELICAN, "The gate failed on one clippy warning. Fix it in the train, or ship with it allowlisted?", { kind: "ask", options: ["Fix in-train", "Ship with allowlist"] });
    await expect(pinned).toBeVisible();
    await expect(page.getByRole("log").getByRole("button", { name: "Fix in-train" })).toBeVisible();
    // One full question in the flow; the composer bookmark carries no choices.
    const question = page.getByRole("log").locator(`.obj[data-notification-id="${ask}"]`);
    await expect(question).toContainText("Fix it in the train, or ship with it allowlisted?");
    await expect(question.getByRole("button")).toHaveCount(2);
    await expect(pinned.locator(".obj")).toHaveCount(0);
    // cas-97d58 F09: the rail and the bookmark read the same open asks. The
    // rail lists the hydrated blocker and the pinned question, marked as pinned.
    await expect(waiting.locator("li")).toHaveCount(2);
    await expect(waiting.locator('.context-jump[data-kind="blocker"]')).toHaveCount(1);
    await expect(waiting.locator('.context-jump[data-kind="ask"]')).toHaveCount(1);
    await expect(waiting.locator('.context-jump[data-kind="ask"] .context-pinned')).toHaveText("Pinned above your reply");
    // The waiting blocker says how it clears, in the thread and in the rail (journey F12).
    await expect(page.getByRole("log").locator('.obj.blk[data-waiting="true"] .blk-hint')).toHaveText("Reply to unblock");
    await expect(waiting.locator('.context-jump[data-kind="blocker"] .context-hint')).toHaveText("Reply to unblock");
    // cas-1f13: the blocker the machine stamped five minutes ahead sorts at its
    // arrival, above the question that came after it, and says its machine's
    // clock is ahead instead of showing a time from the future. cas-8d52
    // (journey F13): it carries no session, so it is this session's own turn:
    // no "session … started" line opens below it, and it is labelled by this
    // session's supervisor.
    const order = await threadOrder(page);
    // cas-d8a5 (journey F32): groups are spoken "cas-src supervisor, …", the codename their description.
    const blockerAt = order.findIndex((label) => label.startsWith("cas-src supervisor, ") && label.endsWith(", machine clock ahead"));
    expect(blockerAt, `blocker marked clock-ahead in ${JSON.stringify(order)}`).toBeGreaterThanOrEqual(0);
    expect(order.filter((label) => label.startsWith("session ")), "the thread is this session's: no session line").toEqual([]);
    await expect(page.getByRole("log").getByRole("group", { name: `Blocker from ${PELICAN}` }).filter({ hasText: "The release gate went red" })).toHaveCount(1);
    // The question arrives live from the same machine, so it is marked the
    // same way a reload would mark it (cas-1f13 review F02).
    // With no session line between them (cas-8d52) the blocker and the
    // question are one turn group under one marked time.
    await expect(page.getByRole("log").locator("time .clock-ahead")).toHaveText([" · machine clock ahead"]);
    const times = await page.getByRole("log").locator(".turn > time").evaluateAll((nodes) => nodes.map((node) => node.firstChild?.textContent ?? ""));
    expect(times, "times read in order down the thread").toEqual([...times].sort());
  });

  await journey.stage("Answer with one tap", async () => {
    const sent = hub.nextSend();
    await page.getByRole("log").getByRole("button", { name: "Fix in-train" }).click();
    expect(await sent).toMatchObject({ text: "Fix in-train", in_reply_to: ask });
    await expect(pinned).toBeHidden();
    // The question stays in the thread with the chosen answer and no open
    // choices; a kept reply carries no visible receipt (cas-97d58 F05).
    await expect(page.getByRole("log").getByRole("group", { name: `Question from ${PELICAN}` }).filter({ hasText: "clippy warning" })).toMatchAriaSnapshot(`
      - group "Question from ${PELICAN}":
        - paragraph: /clippy warning/
        - text: Fix in-train
    `);
    await expect(page.getByRole("button", { name: "Ship with allowlist" })).toHaveCount(0);
    // Handled, the question quiets to the supervisor's colour and keeps the
    // tick on the chosen answer (journey F12); the answer also acknowledged
    // the earlier blocker, which drops its hint.
    const answered = page.getByRole("log").locator('.obj.t-a[data-answered="true"]');
    const quiet = await answered.evaluate((object) => {
      const probe = document.createElement("span"); probe.style.background = "var(--sup-bg)"; object.append(probe);
      const supervisor = getComputedStyle(probe).backgroundColor; probe.remove();
      const body = getComputedStyle(object.querySelector(".obj-body")!);
      return { body: body.backgroundColor, edge: body.borderLeftColor, supervisor };
    });
    expect(quiet.body, "answered question background").toBe(quiet.supervisor);
    expect(quiet.edge, "no attention edge").toBe("rgba(0, 0, 0, 0)");
    await expect(answered.locator(".chip.sent .tick")).toBeVisible();
    await expect(page.getByRole("log").locator(".blk-hint")).toHaveCount(0);
    // cas-71af (aac8 QA F01): the earlier blocker stops waiting too. It
    // quiets to the supervisor's colour instead of staying a red alarm, but
    // the answer went to the question, not to it: it says only that the
    // operator has written since, never that they replied (cas-e829).
    const handled = page.getByRole("log").getByRole("group", { name: `Blocker from ${PELICAN}, you've written since` });
    await expect(handled.locator(".blk-handled")).toHaveText("You've written since this");
    await expect(handled.locator(".blk-handled .tick")).toHaveCount(0);
    expect(await handled.evaluate((object) => {
      const probe = document.createElement("span"); probe.style.background = "var(--sup-bg)"; object.append(probe);
      const supervisor = getComputedStyle(probe).backgroundColor; probe.remove();
      return getComputedStyle(object.querySelector(".obj-body")!).backgroundColor === supervisor;
    }), "acknowledged blocker background").toBe(true);
    // The answer comes after everything shown, so nothing is left waiting in the rail (cas-ce17).
    await expect(waiting).toBeHidden();
    // It sorts after the machine's future-stamped blocker, but shows the time it was sent, under today (cas-ac1f).
    const sent5 = await lastSend(page);
    expect(youNow()).toContain(sent5.label);
    expect(sent5.day).toBe("Today");
  });

  await journey.stage("See the supervisor act on the answer", async () => {
    hub.answerLatest(PELICAN, "Fixing the warning in the train now.");
    await expect(page.getByRole("log").getByText("Fixing the warning in the train now.")).toBeVisible();
    // A new blocker after the answer does wait: the answer does not reach forward in time.
    hub.supervisorSays(PELICAN, "A second gate went red; the train is still held.", { kind: "blocker" });
    await expect(waiting.locator("li")).toHaveCount(1);
    await expect(waiting).toContainText("A second gate went red");
    await expect(waiting.locator(".context-hint")).toHaveText("Reply to unblock");
    await expect(page.getByRole("log").locator('.obj.blk[data-waiting="true"]')).toContainText("A second gate went red");
    await expect(page.getByRole("log").locator(".blk-hint")).toHaveText(["Reply to unblock"]);
  });

  await journey.stage("Jump to a long question and dismiss its bookmark", async () => {
    const composer = page.getByRole("textbox", { name: "Your message" });
    const release = hub.supervisorSays(PELICAN, "Every lane is merged and the gate is green.\n\n- 21 tasks closed.\n- The diff is 579 files.\n- The audit docs ship with it.\n- **Ask:** open the PR to main and cut a release?", { kind: "ask", options: ["Open PR", "Hold release"] });
    const copy = page.getByRole("log").locator(`.obj[data-notification-id="${release}"]`);
    const bar = pinned.locator(".pinned-expand");
    await expect(copy.getByRole("button", { name: "Open PR", exact: true })).toBeVisible();
    hub.supervisorSays(PELICAN, "FYI: the Mac tests are still running.");
    hub.supervisorSays(PELICAN, "Gate 2 of 3 is going.", { kind: "status" });
    await expect(page.getByRole("log").getByText("FYI: the Mac tests are still running.")).toBeVisible();
    await expect(copy).not.toHaveAttribute("data-retired", /.+/);
    await expect(bar).toHaveText("Waiting on you: open the PR to main and cut a release?");
    await expect(pinned.getByRole("button", { name: "Open PR", exact: true })).toHaveCount(0);
    const desktop = page.viewportSize()!;
    await page.emulateMedia({ colorScheme: "dark" }); await page.setViewportSize({ width: 390, height: 844 });
    await composer.focus();
    const barBox = (await bar.boundingBox())!;
    expect(barBox.height).toBeLessThanOrEqual(48); expect(barBox.height).toBeGreaterThanOrEqual(44);
    await page.setViewportSize({ width: 390, height: 440 });
    const thread = page.locator(".conversation-reading.thread");
    const room = await thread.evaluate(element => ({ height: element.clientHeight, line: parseFloat(getComputedStyle(element.querySelector(".msgs .bub p, .msgs .obj p")!).lineHeight) }));
    expect(room.height).toBeGreaterThanOrEqual(3 * room.line);
    await bar.click();
    await expect(page.locator(`[data-key="reply:${release}"]`)).toBeFocused();
    await expect(pinned.locator(".obj")).toHaveCount(0);
    await page.setViewportSize({ width: 390, height: 844 });
    await swipeAway(page, ".pinned-ask", -260);
    await expect(pinned).toBeHidden();
    await expect(copy).toHaveAttribute("data-retired", "dismissed");
    await expect(copy.locator(".ask-retired")).toHaveText("Dismissed. You can still answer here.");
    await expect(copy.getByRole("button", { name: "Open PR", exact: true })).toBeVisible();
    await page.emulateMedia({ colorScheme: "light" }); await page.setViewportSize(desktop);
  });

  await journey.stage("Reply to a machine a day ahead", async () => {
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /gabber-studio/ }).click();
    await expect(page.getByRole("log").getByText("Mac build is queued behind the nightly.")).toBeVisible();
    const composer = page.getByRole("textbox", { name: "Your message" });
    await composer.fill("Thanks — ping me when it starts.");
    const sent = hub.nextSend();
    await page.getByRole("button", { name: "Send to the gabber-studio supervisor", exact: true }).click();
    await sent;
    // cas-1f13: the machine's turn is not filed under tomorrow above Today. It
    // sits under Today at its arrival, marked "machine clock ahead", and the
    // send follows it under today at the time it was sent.
    const reply = await lastSend(page);
    expect(youNow()).toContain(reply.label);
    expect(reply.day).toBe("Today");
    await expect(page.getByRole("log").locator(".day")).toHaveText(["Today"]);
    const order = await threadOrder(page);
    const machineTurn = order.findIndex((label) => label.startsWith("gabber-studio supervisor, ") && label.endsWith(", machine clock ahead"));
    expect(machineTurn, `machine turn marked clock-ahead in ${JSON.stringify(order)}`).toBeGreaterThanOrEqual(0);
    expect(order.lastIndexOf(reply.label), "the send sits below the machine's turn").toBeGreaterThan(machineTurn);
    // cas-8d52 (journey F13): the machine's sessionless turn is this session's; no line splits it from the send.
    expect(order.filter((label) => label.startsWith("session ")), JSON.stringify(order)).toEqual([]);
  });

  await journey.stage("Reopen the page", async () => {
    // cas-1f13 review F01: a reload rebuilds the thread from history alone,
    // with the machine's stamps. Every turn keeps the place the visit gave it,
    // in the machine's sequence, and no time reads later than a turn below it.
    const list = page.getByRole("navigation", { name: "Choose a supervisor" });
    const shape = (labels: string[]) => labels.map((label) => label.replace(/\d{1,2}:\d{2}/g, "HH:MM"));
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("log").getByText("A second gate went red; the train is still held.")).toBeVisible();
    const before = await threadOrder(page);
    // cas-8d52 (journey F11): the reload comes minutes later; every turn keeps
    // the time the visit showed, not the reload's.
    await page.clock.fastForward(180_000);
    await page.reload();
    await list.getByRole("button", { name: /cas-src/ }).click();
    await expect(page.getByRole("log").getByText("A second gate went red; the train is still held.")).toBeVisible();
    const after = await threadOrder(page);
    expect(shape(after), JSON.stringify({ before, after })).toEqual(shape(before));
    expect(after, "the same times, not only the same order").toEqual(before);
    await expect(page.getByRole("log").locator(".day")).toHaveText(["Today"]);
    const times = await page.getByRole("log").locator(".turn > time").evaluateAll((nodes) => nodes.map((node) => node.firstChild?.textContent ?? ""));
    expect(times, "times read in order down the thread").toEqual([...times].sort());
  });
});

test("HUB-J7 a machine clock ahead: the first visit and a reload agree, and the row ages (cas-9e33, cas-24fe)", journeyPart, async ({ page, journey }) => {
  // The machine's clock runs five minutes ahead, and nothing in the thread has
  // shown it yet: no history, one answer arriving live.
  // The machine's clock moves with the browser's: when the page skips ahead,
  // so do its stamps. Otherwise a stored copy that lands before the live answer
  // dates that answer by a stamp the skip left behind.
  let skipped = 0;
  const time = { now: () => journeyNow() + skipped, delay: (callback: () => void, ms: number) => void setTimeout(callback, ms) };
  const skip = async (ms: number) => { await page.clock.fastForward(ms); skipped += ms; };
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"], clockAheadMs: { [PELICAN]: 300_000 }, time });
  const list = page.getByRole("navigation", { name: "Choose a supervisor" });
  const log = page.getByRole("log");
  let before: string[] = [];

  await journey.stage("The supervisor answers live", async () => {
    await journey.open();
    await openConversation(page, "cas-src");
    await page.getByRole("textbox", { name: "Your message" }).fill("Is the gate green?");
    const sent = hub.nextSend();
    await activate(page, page.getByRole("button", { name: "Send to the cas-src supervisor", exact: true }));
    await sent;
    hub.answerLatest(PELICAN, "Yes, the gate is green.");
    await expect(log.getByText("Yes, the gate is green.")).toBeVisible();
    // The visit cannot know the machine's lead yet: the answer shows its arrival, unmarked.
    await expect(log.locator("time .clock-ahead")).toHaveCount(0);
    before = await threadOrder(page);
    await showConversationList(page);
    await expect(list.locator(".conversation-when")).toHaveText("now");
  });

  await journey.stage("Reload three minutes later", async () => {
    await skip(180_000);
    await page.reload();
    await openConversation(page, "cas-src");
    await expect(log.getByText("Yes, the gate is green.")).toBeVisible();
    // cas-9e33: the reload rebuilds the answer from the machine's stamp and
    // shows it exactly as the visit did: the same time, and no mark the visit
    // never showed.
    await expect(log.locator("time .clock-ahead")).toHaveCount(0);
    expect(await threadOrder(page), "the reload shows the thread the visit showed").toEqual(before);
    // cas-24fe: the row dates the answer from its arrival, not from the
    // machine's stamp in this browser's future.
    await showConversationList(page);
    await expect(list.locator(".conversation-when")).toHaveText("3m");
  });

  await journey.stage("Come back five minutes later", async () => {
    await skip(300_000);
    await expect(list.locator(".conversation-when")).toHaveText("8m");
    // The reload measured the lead, so the next live answer says the clock is ahead.
    await openConversation(page, "cas-src");
    hub.answerLatest(PELICAN, "Tagging 3.26.0 now.");
    await expect(log.getByText("Tagging 3.26.0 now.")).toBeVisible();
    await expect(log.locator("time .clock-ahead")).toHaveText([" · machine clock ahead"]);
    await showConversationList(page);
    await expect(list.locator(".conversation-when")).toHaveText("now");
  });
});

// The waiting bookmark never turns into a choice tray under a double tap.
test("HUB-J7 double-tapping the bookmark jumps without answering (cas-450b, cas-6e3a)", journeyPart, async ({ page, journey }) => {
  const hub = await journey.hub({ machines: [ATLAS], paired: ["atlas"] });
  let ask = 0;
  await journey.stage("Read a question on a short phone screen", async () => {
    await page.setViewportSize({ width: 390, height: 440 }); await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /cas-src/ }).click();
    ask = hub.supervisorSays(PELICAN, "Open the PR and cut a release?", { kind: "ask", options: ["Open PR", "Hold release"] });
    await expect(page.locator(".pinned-expand")).toBeVisible();
  });
  await journey.stage("A double tap jumps without sending", async () => {
    const before = hub.sends.length;
    await page.locator(".pinned-expand").dblclick();
    await expect(page.locator(`[data-key="reply:${ask}"]`)).toBeFocused();
    expect(hub.sends.slice(before)).toEqual([]);
    await expect(page.locator(".pinned-ask .obj")).toHaveCount(0);
  });
  await journey.stage("Deliberately choose an option in the thread", async () => {
    const sent = hub.nextSend();
    await page.getByRole("log").getByRole("button", { name: "Open PR", exact: true }).click();
    expect(await sent).toMatchObject({ text: "Open PR", in_reply_to: ask });
    await expect(page.locator(".pinned-ask")).toBeHidden();
  });
});
