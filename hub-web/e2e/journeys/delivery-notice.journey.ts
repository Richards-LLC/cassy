import { test, expect } from "./journey";
import { journeyDay, journeyStamp } from "./clock";
import type { Machine } from "./hub-double";

// cas-e829: the operator's 2026-10-01 Accounting screenshot. The live session
// has a blocker from yesterday, a later message from the operator that does
// not answer it, and a relay-watchdog notice that the supervisor never saw an
// update. The notice is plumbing: it belongs in attention, not the thread.
const SESSION = "Accounting-rapid-gazelle-52";
const SUPERVISOR = "happy-cheetah-1";
const SUBJECT = 3196290;
const ATLAS: Machine = {
  id: "atlas",
  label: "Atlas · Linux",
  sessions: [{ name: SESSION, supervisor: SUPERVISOR, project_dir: "/projects/Accounting", workers: [], liveness: "live" }],
};
const NOTICE_SUMMARY = "Supervisor hasn't seen: worker died: daring-robin-43 (9m)";
const NOTICE_TEXT = "The supervisor (happy-cheetah-1, Codex) was told 9 minutes ago that worker died: daring-robin-43, and the message never reached it.";
const notice = (id: number, at: string, resolved = false) => ({
  notification_id: id, reply_to: null, message: NOTICE_TEXT, summary: NOTICE_SUMMARY, device_id: "*", kind: "blocker", attachments: [], session: SESSION, at,
  notice: { source: "relay-watchdog", subject: SUBJECT, resolved },
});

test("HUB-J15 see a delivery problem as attention, not conversation", async ({ page, journey }) => {
  // Ten stages, two of them waiting out a catalog heartbeat: give it the same
  // headroom HUB-J3 has under a loaded factory host.
  test.setTimeout(120_000);
  const hub = await journey.hub({
    machines: [ATLAS],
    paired: ["atlas"],
    history: {
      [SESSION]: [{ has_earlier: false,
        messages: [{ notification_id: 902, target: SUPERVISOR, text: "Looking at the ledger import now.", state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Pixel 10", session: SESSION, at: journeyStamp(-60 * 60_000) }],
        replies: [
          { notification_id: 900, reply_to: null, message: "The ledger import is red: 3 rows failed validation.", summary: "", device_id: "journey-device", kind: "blocker", attachments: [], session: SESSION, at: journeyDay(1, 17, 20) },
          notice(901, journeyDay(1, 17, 49)),
        ] }],
    },
  });
  const log = page.getByRole("log");
  const attentionItems = page.locator("#attention-panel article", { hasText: NOTICE_SUMMARY });

  await journey.stage("Open the session: only its conversation", async () => {
    await journey.open();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /Accounting/ }).click();
    await expect(log).toContainText("The ledger import is red");
    await expect(log).toContainText("Looking at the ledger import now.");
    await expect(log).not.toContainText("never reached it");
    // Yesterday's turn shows its date, not a bare clock time.
    await expect(log.locator(".turn time").first()).toContainText("Sep 29, 17:20");
  });

  await journey.stage("A blocker I never answered does not say I replied", async () => {
    const blocker = log.locator('.obj.blk[data-notification-id="900"]');
    await expect(blocker.locator(".blk-handled")).toHaveText("You've written since this");
    await expect(blocker).not.toContainText("you replied");
    await expect(blocker.locator(".blk-handled svg.tick")).toHaveCount(0);
  });

  await journey.stage("The delivery problem is one attention item", async () => {
    await expect(attentionItems).toHaveCount(1);
    // The watchdog raises it again (a repeat and a live replay): still one.
    hub.supervisorSays(SESSION, NOTICE_TEXT, { kind: "blocker", summary: NOTICE_SUMMARY, device_id: "*", notice: { source: "relay-watchdog", subject: SUBJECT, resolved: false } });
    await page.waitForTimeout(300);
    await expect(attentionItems).toHaveCount(1);
    await expect(log).not.toContainText("never reached it");
    // cas-5c22: the item carries the notice's own time, not when this page heard of it.
    await expect(attentionItems.locator("time")).toHaveText("Sep 29, 17:49");
  });

  const desktop = page.viewportSize()!;
  await journey.stage("On a phone, the delivery problem is one tap from the conversation", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    const badge = page.getByRole("button", { name: "Attention: 1 item for this session" });
    await expect(badge).toBeVisible();
    await expect(badge).toHaveText("1");
    await badge.click();
    const sheet = page.getByRole("dialog", { name: "Attention for this session" });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Close attention" })).toBeFocused();
    await expect(sheet.locator("article", { hasText: NOTICE_SUMMARY })).toBeVisible();
    await expect(sheet.locator("article", { hasText: NOTICE_SUMMARY }).locator("time")).toHaveText("Sep 29, 17:49");
  });

  await journey.stage("Keyboard stays in the sheet", async () => {
    const sheet = page.getByRole("dialog", { name: "Attention for this session" });
    // cas-a5c6: the modal sheet holds Tab and Shift+Tab; nothing behind it is reachable.
    const inSheet = () => page.evaluate(() => Boolean(document.activeElement?.closest(".conversation-context[role='dialog']")));
    // Each element gets a stable tag the first time it is focused, so two
    // controls with the same words still count as two stops.
    const focused = () => page.evaluate(() => {
      const node = document.activeElement as HTMLElement | null;
      if (!node) return "";
      const label = `${node.tagName.toLowerCase()}.${node.className.split(" ")[0]}:${node.textContent?.trim() ?? ""}`;
      const counter = window as unknown as { __stop?: number };
      node.dataset.stop ??= String((counter.__stop = (counter.__stop ?? 0) + 1));
      return label === "button.context-sheet-close:×" || label === "summary.:Details" ? label : `${label}#${node.dataset.stop}`;
    });
    // QA F01: Shift+Tab from Close really moves, to the sheet's last stop (the
    // collapsed Details' summary, not the Copy button hidden inside it).
    await page.keyboard.press("Shift+Tab");
    expect(await inSheet(), "Shift+Tab from Close stays in the sheet").toBe(true);
    expect(await focused()).toBe("summary.:Details");
    // Tab walks every stop once and wraps from Details back to Close.
    const stops: string[] = [];
    for (let step = 0; step < 12; step += 1) {
      await page.keyboard.press("Tab");
      expect(await inSheet(), `Tab ${step + 1} stays in the sheet`).toBe(true);
      stops.push(await focused());
    }
    const close = "button.context-sheet-close:×";
    expect(stops[0], "Tab from Details wraps to Close").toBe(close);
    const lap = stops.indexOf(close, 1);
    expect(lap, `Tab comes back round to Close: ${stops.join(" → ")}`).toBeGreaterThan(1);
    expect(new Set(stops.slice(0, lap)).size, `each Tab moves on: ${stops.join(" → ")}`).toBe(lap);
    // QA F03: a heartbeat redraw (every 5 s) never moves the keyboard user.
    while (!(await focused()).startsWith("button.attention-dismiss-group")) await page.keyboard.press("Tab");
    const resting = await focused();
    // A catalog refetch and redraw is driven by a machine event (cas-9772), not waited out.
    await hub.announceCatalog("atlas");
    await hub.announceCatalog("atlas");
    expect(await focused(), "focus stays put across a heartbeat").toBe(resting);
    // QA round 3 F02/F03: a minute rollover (the cards' ages change) is not a
    // redraw either. Open the notice's Details, rest on its Copy, cross a
    // minute on the page clock: the same element keeps focus, Details stays open.
    const notice = sheet.locator("article.attention-item").first();
    await notice.locator("summary").click();
    await notice.getByRole("button", { name: "Copy" }).focus();
    const copy = await focused();
    await page.clock.fastForward(61_000);
    await hub.announceCatalog("atlas");
    expect(await focused(), "focus stays on Copy across a minute").toBe(copy);
    await expect(notice.locator("details")).toHaveAttribute("open", "");
    await sheet.locator(".attention-dismiss-group").focus();
    await expect(page.locator(".conversation-main")).toHaveAttribute("inert", "");
    await expect(sheet).toBeVisible();
  });

  await journey.stage("A palette opened over the sheet closes first", async () => {
    const sheet = page.getByRole("dialog", { name: "Attention for this session" });
    const palette = page.locator("#command-palette");
    // QA F02: the topmost layer owns Escape.
    await page.keyboard.press("ControlOrMeta+k");
    if (!(await palette.evaluate((node) => (node as HTMLDialogElement).open))) await page.keyboard.press("ControlOrMeta+k");
    await expect(palette).toHaveAttribute("open", "");
    await expect(page.getByRole("searchbox", { name: "Filter commands" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(palette).not.toHaveAttribute("open", "");
    await expect(sheet).toBeVisible();
    // Focus is back in the sheet, on the control it left (Dismiss group).
    await expect(sheet.locator(".attention-dismiss-group")).toBeFocused();
  });

  await journey.stage("Close it and keep reading", async () => {
    const sheet = page.getByRole("dialog", { name: "Attention for this session" });
    // Escape closes it from anywhere, even with focus dropped to the page.
    await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await page.keyboard.press("Escape");
    await expect(sheet).toBeHidden();
    const badge = page.getByRole("button", { name: "Attention: 1 item for this session" });
    await expect(badge).toBeFocused();
    // A catalog change rebuilds the shell, and the fresh badge starts hidden
    // until its region shows it: focus must still land back on it.
    const sessions = hub.machine("atlas").sessions;
    sessions.push({ name: "Accounting-quiet-otter-3", supervisor: "quiet-otter-3", project_dir: "/projects/Ledger", workers: [], liveness: "live" });
    await hub.announceCatalog("atlas");
    await expect(page.getByRole("navigation", { name: "Choose a supervisor", includeHidden: true }).locator(".conversation-row")).toHaveCount(2);
    sessions.pop();
    await expect(badge).toBeFocused();
    await expect(log).toContainText("The ledger import is red");
  });

  await journey.stage("A sheet left open on a phone is a plain rail on a desktop", async () => {
    const badge = page.getByRole("button", { name: "Attention: 1 item for this session" });
    await badge.click();
    await expect(page.getByRole("dialog", { name: "Attention for this session" })).toBeVisible();
    // cas-a5c6: the phone turns into a desktop (rotation, or a window resize).
    await page.setViewportSize(desktop);
    const rail = page.locator(".conversation-context");
    await expect(rail).not.toHaveAttribute("role", "dialog");
    await expect(rail).not.toHaveAttribute("aria-modal", "true");
    await expect(rail).toHaveAttribute("aria-label", "Conversation context");
    await expect(page.locator("#conversation-attention")).toHaveAttribute("aria-expanded", "false");
    await expect(page.locator("[inert]")).toHaveCount(0);
    await expect(attentionItems).toHaveCount(1);
  });

  await journey.stage("It retires once the update gets through", async () => {
    await page.setViewportSize(desktop);
    hub.send(SESSION, { OperatorNoticeResolved: { notification_id: 901, subject: SUBJECT } });
    await expect(attentionItems).toHaveCount(0);
    // After a reload the resolved notice does not come back.
    await page.reload();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /Accounting/ }).click();
    await expect(log).toContainText("The ledger import is red");
    await expect(attentionItems).toHaveCount(0);
  });

  await journey.stage("An answer to an earlier session's question stays here", async () => {
    // The supervisor answers a question asked in yesterday's ended session:
    // it arrives in this thread, naming the session it answers.
    hub.supervisorSays(SESSION, "The bank feed reconciled overnight.", { reply_to: 3196200, reply_to_session: "Accounting-wise-lion-31" });
    const answer = log.locator(".bub", { hasText: "The bank feed reconciled overnight." });
    await expect(answer.locator(".reply-quote")).toHaveText("re: earlier session wise-lion-31");
    await expect(answer).toHaveAttribute("data-reply-to", "3196200");
    await expect(page.getByRole("region", { name: "Earlier sessions" })).toBeHidden();
  });
});
