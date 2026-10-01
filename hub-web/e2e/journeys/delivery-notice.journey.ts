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
  });

  await journey.stage("It retires once the update gets through", async () => {
    hub.send(SESSION, { OperatorNoticeResolved: { notification_id: 901, subject: SUBJECT } });
    await expect(attentionItems).toHaveCount(0);
    // After a reload the resolved notice does not come back.
    await page.reload();
    await page.getByRole("navigation", { name: "Choose a supervisor" }).getByRole("button", { name: /Accounting/ }).click();
    await expect(log).toContainText("The ledger import is red");
    await expect(attentionItems).toHaveCount(0);
  });
});
