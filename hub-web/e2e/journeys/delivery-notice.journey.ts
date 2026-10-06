import { test, expect, expectWholeFocusRing, RECEIPTS } from "./journey";
import { join } from "node:path";
import { journeyDay, journeyStamp } from "./clock";
import type { Machine } from "./hub-double";
import { expectDetailsCopyRow } from "../details-copy-row";

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
const NOTICE_HEADLINE = "The supervisor missed an update: a worker stopped";
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
        messages: [{ notification_id: 3196200, target: SUPERVISOR, text: "Did the bank feed reconcile?\nPlease check the ledger.", state: "acknowledged", stamped: true, device_id: "journey-device", session: "Accounting-wise-lion-31", at: journeyDay(1, 16, 50) }, { notification_id: 902, target: SUPERVISOR, text: "Looking at the ledger import now.", state: "acknowledged", stamped: true, device_id: "journey-device", operator_label: "Pixel 10", session: SESSION, at: journeyStamp(-60 * 60_000) }],
        replies: [
          { notification_id: 900, reply_to: null, message: "The ledger import is red: 3 rows failed validation.", summary: "", device_id: "journey-device", kind: "blocker", attachments: [], session: SESSION, at: journeyDay(1, 17, 20) },
          notice(901, journeyDay(1, 17, 49)),
        ] }],
    },
  });
  const log = page.getByRole("log");
  const attentionItems = page.locator("#attention-panel article", { hasText: NOTICE_HEADLINE });

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
    await expect(attentionItems.locator(".attention-title")).toHaveText(NOTICE_HEADLINE);
    await expect(attentionItems.locator(".attention-detail")).toHaveText("Worker daring-robin-43 stopped. The update did not reach the supervisor.");
    await expect(page.locator(".attention-group-label")).toHaveText("Accounting · happy-cheetah-1");
    await expect(page.locator(".attention-dismiss-group")).toHaveCount(0);
  });

  const desktop = page.viewportSize()!;
  await journey.stage("On a phone, the delivery problem is one tap from the conversation", async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    const badge = page.getByRole("button", { name: "Attention: 1 item for this session" });
    await expect(badge).toBeVisible();
    await expect(badge).toHaveText("1");
    // cas-97d58 F07: with the attention chip, Raw output and Interrupt on the
    // row, the identity takes its own line; title and machine read whole.
    const identity = page.locator(".conversation-identity");
    const whole = await identity.evaluate((node) => {
      const clipped = (selector: string) => { const element = node.querySelector<HTMLElement>(selector); return element ? element.scrollWidth > element.clientWidth + 1 : false; };
      return { title: !clipped("h1 b"), machine: !clipped(".host-machine") && !clipped(".host-where"), row: getComputedStyle(node).gridRowStart };
    });
    expect(whole).toEqual({ title: true, machine: true, row: "2" });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no sideways scroll").toBe(true);
    await page.screenshot({ path: join(RECEIPTS, "phone-header-attention-390.png") });
    await badge.click();
    const sheet = page.getByRole("dialog", { name: "Attention for this session" });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByRole("button", { name: "Close attention" })).toBeFocused();
    await expect(sheet.locator("article", { hasText: NOTICE_HEADLINE })).toBeVisible();
    await expect(sheet.locator("article", { hasText: NOTICE_HEADLINE }).locator("time")).toHaveText("Sep 29, 17:49");
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
    while (!(await focused()).startsWith("button.attention-dismiss")) await page.keyboard.press("Tab");
    const resting = await focused();
    // A catalog refetch and redraw is driven by a machine event (cas-9772), not waited out.
    await hub.announceCatalog("atlas");
    await hub.announceCatalog("atlas");
    expect(await focused(), "focus stays put across a heartbeat").toBe(resting);
    // QA round 3 F02/F03: a minute rollover (the cards' ages change) is not a
    // redraw either. Open the notice's Details, rest on its Copy, cross a
    // minute on the page clock: the same element keeps focus, Details stays open.
    const notice = sheet.locator("article.attention-item").first();
    // cas-b56b: while Details is closed its Copy is not laid out at all, so
    // no "Copy" box sits below the rail's scroll range for a layout check
    // (the strict visual inspector) to find. Opening Details brings it back.
    expect(await notice.locator(".attention-copy").evaluate((node) => node.getBoundingClientRect().height), "a closed notice's Copy has no box").toBe(0);
    await notice.locator("summary").click();
    await expectDetailsCopyRow(notice, "Copy has its own row, above the full Details text at390px");
    await expect(notice.locator("pre")).toContainText("9 minutes ago");
    // cas-ed87: Details reads as the message, never as a {summary, message} object.
    await expect(notice.locator("pre"), "Details is the message as text at 390px").toHaveText(`${NOTICE_SUMMARY}\n\n${NOTICE_TEXT}`);
    await notice.locator("summary").focus();
    await page.keyboard.press("Tab");
    await expectWholeFocusRing(notice.getByRole("button", { name: "Copy" }), { vertical: true });
    const copy = await focused();
    await page.clock.fastForward(61_000);
    await hub.announceCatalog("atlas");
    expect(await focused(), "focus stays on Copy across a minute").toBe(copy);
    await expect(notice.locator("details")).toHaveAttribute("open", "");
    await notice.getByRole("button", { name: "Copy", exact: true }).click();
    expect(await page.evaluate(() => navigator.clipboard.readText()), "Copy still copies the displayed Details after refresh").toBe(await notice.locator("pre").textContent());
    // cas-f486: a phone that wakes after ten minutes, with the session list
    // changed meanwhile, rebuilds the whole page. The rebuilt sheet has the
    // same Details open and the same Copy focused, in new elements.
    await page.evaluate(() => document.querySelector(".conversation-shell")!.setAttribute("data-before-wake", ""));
    const atlas = hub.machine("atlas").sessions;
    atlas.push({ name: "Accounting-calm-wren-4", supervisor: "calm-wren-4", project_dir: "/projects/Payroll", workers: [], liveness: "live" });
    await page.clock.fastForward(600_000);
    await hub.announceCatalog("atlas");
    await expect(page.locator(".conversation-shell[data-before-wake]")).toHaveCount(0);
    const sameControl = (label: string) => label.replace(/#\d+$/, "");
    await expect.poll(async () => sameControl(await focused()), { message: "focus is on Copy again after the page is rebuilt" }).toBe(sameControl(copy));
    await expect(sheet.locator("article.attention-item").first().locator("details")).toHaveAttribute("open", "");
    atlas.pop();
    await hub.announceCatalog("atlas");
    await expect.poll(async () => sameControl(await focused())).toBe(sameControl(copy));
    await sheet.locator(".attention-dismiss").focus();
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
    // Focus is back in the sheet, on the control it left (Dismiss notice).
    await expect(sheet.locator(".attention-dismiss")).toBeFocused();
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
    // Nothing the operator can see is left inert. The supervisor pane's host
    // is inert by design: it is hidden plumbing, never on screen (cas-0546).
    await expect(page.locator("[inert]:not(.pane-host)")).toHaveCount(0);
    await expect(page.locator(".pane-host")).toBeHidden();
    await expect(attentionItems).toHaveCount(1);
  });

  await journey.stage("It retires once the update gets through", async () => {
    await page.setViewportSize(desktop);
    // cas-94b6: force the catalog redraw that previously landed during Copy's
    // boundingBox call. A retained node loses its box, while the current
    // Details stays open and must still satisfy the same layout assertion.
    const staleCopy = await attentionItems.getByRole("button", { name: "Copy" }).elementHandle();
    const sessions = hub.machine("atlas").sessions;
    sessions.push({ name: "Accounting-bright-lark-8", supervisor: "bright-lark-8", project_dir: "/projects/Audit", workers: [], liveness: "live" });
    await hub.announceCatalog("atlas");
    await expect.poll(() => staleCopy!.evaluate((node) => node.isConnected)).toBe(false);
    expect(await staleCopy!.boundingBox(), "the replaced Copy has no geometry").toBeNull();
    await expect(attentionItems.locator("details")).toHaveAttribute("open", "");
    await expectDetailsCopyRow(attentionItems, "Copy has its own row at1280px too after a catalog redraw");
    sessions.pop();
    await hub.announceCatalog("atlas");
    await expectDetailsCopyRow(attentionItems, "Copy keeps its row after the catalog settles");
    await expect(attentionItems.locator("pre"), "Details is the message as text at 1280px").toHaveText(`${NOTICE_SUMMARY}\n\n${NOTICE_TEXT}`);
    // cas-177c: Copy says what it copied, the Details text, not an "event payload".
    await attentionItems.getByRole("button", { name: "Copy" }).click();
    await expect(page.locator("#toast")).toHaveText("Details copied");
    expect(await page.evaluate(() => navigator.clipboard.readText()), "Copy copies the Details text").toBe(`${NOTICE_SUMMARY}\n\n${NOTICE_TEXT}`);
    hub.send(SESSION, { OperatorNoticeResolved: { notification_id: 901, subject: SUBJECT } });
    await expect(attentionItems).toHaveCount(0);
    // After a reload the resolved notice does not come back.
    await page.reload();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /Accounting/ }).click();
    await expect(log).toContainText("The ledger import is red");
    await expect(attentionItems).toHaveCount(0);
    // cas-b113: resolved watchdog notices must not return from the device
    // journal as conversation blockers, nor make the rail wait on me.
    await expect(log.locator(".bub", { hasText: NOTICE_TEXT })).toHaveCount(0);
    await expect(log).not.toContainText("never reached it");
    await expect(page.locator('.conversation-context .context-jump[data-kind="blocker"]')).toHaveCount(0);
    const waiting = page.locator('.conversation-context [data-section="waiting"]');
    await expect(waiting).toBeHidden();
    await expect(waiting.locator("li")).toHaveCount(0);
    // Hidden static headings do not represent a waiting state. The spoken
    // rail must contain neither a waiting heading nor a waiting region.
    expect(await page.locator(".conversation-context").ariaSnapshot()).not.toMatch(/Waiting on you/);
    await expect(page.getByRole("region", { name: /Waiting on you/ })).toHaveCount(0);
  });

  await journey.stage("An answer to an earlier session's question stays here", async () => {
    // The supervisor answers a question asked in yesterday's ended session:
    // it arrives in this thread, naming the session it answers.
    hub.supervisorSays(SESSION, "The bank feed reconciled overnight.", { reply_to: 3196200, reply_to_session: "Accounting-wise-lion-31" });
    const answer = log.locator(".bub", { hasText: "The bank feed reconciled overnight." });
    await expect(answer.locator(".reply-quote")).toHaveText("Reply to “Did the bank feed reconcile?” · wise-lion-31");
    await expect(answer).toHaveAttribute("data-reply-to", "3196200");
    await expect(page.getByRole("region", { name: "Earlier sessions" }).locator("details[open]")).toHaveCount(0);
  });
});
