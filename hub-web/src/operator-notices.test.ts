import { describe, expect, it } from "vitest";
import { attentionContent, attentionTimeLabel, createAttentionItem } from "./attention";
import { conversationAttentionBadge } from "./conversation-shell";
import { isOperatorNotice, NOTICE_KIND, noticeFingerprint, noticeTime, planNotice } from "./operator-notices";
import type { OperatorReply } from "./types";

// cas-e829: the operator's 2026-10-01 Accounting screenshot, row 3196301.
const watchdog = (notice: OperatorReply["notice"]): OperatorReply => ({
  notification_id: 3196301,
  reply_to: null,
  message: "The supervisor (sharp-stork-98, Claude) was told 9 minutes ago that worker died: daring-robin-43, and the message never reached it.",
  summary: "Supervisor hasn't seen: worker died: daring-robin-43 (9m)",
  device_id: "*",
  kind: "blocker",
  notice,
});

describe("system notices (cas-e829)", () => {
  it("tells a notice from a supervisor turn", () => {
    expect(isOperatorNotice(watchdog({ source: "relay-watchdog", subject: 3196290 }))).toBe(true);
    expect(isOperatorNotice(watchdog(undefined))).toBe(false);
    expect(isOperatorNotice(watchdog(null))).toBe(false);
  });

  it("raises one attention item per unseen relay, however often it is replayed", () => {
    const known = new Set<string>();
    const first = planNotice("atlas", "Accounting-rapid-gazelle-52", watchdog({ source: "relay-watchdog", subject: 3196290 }), (fp) => known.has(fp));
    expect(first).toMatchObject({ action: "raise", fingerprint: "notice:atlas:Accounting-rapid-gazelle-52:3196290" });
    if (first.action !== "raise") throw new Error("expected a raise");
    expect(first.content).toMatchObject({ headline: "The supervisor missed an update: a worker stopped", severity: "warning", action: "view_pane" });
    known.add(first.fingerprint);
    // The same relay again (a reload replays history): nothing new.
    expect(planNotice("atlas", "Accounting-rapid-gazelle-52", { ...watchdog({ source: "relay-watchdog", subject: 3196290 }), notification_id: 3196302 }, (fp) => known.has(fp)).action).toBe("none");
  });

  it("retires the item once the relay is resolved", () => {
    expect(planNotice("atlas", "s", watchdog({ source: "relay-watchdog", subject: 7, resolved: true }), () => true)).toEqual({ action: "resolve", fingerprint: "notice:atlas:s:7" });
    // A resolution announced on its own lands on the same item.
    expect(noticeFingerprint("atlas", "s", 3196301, 7)).toBe("notice:atlas:s:7");
    // A notice about no particular row is keyed by itself.
    expect(noticeFingerprint("atlas", "s", 3196301)).toBe("notice:atlas:s:3196301");
  });

  it("reads as a warning about the supervisor in the attention lane", () => {
    const plan = planNotice("atlas", "s", watchdog({ source: "relay-watchdog", subject: 7 }), () => false);
    if (plan.action !== "raise") throw new Error("expected a raise");
    const item = createAttentionItem({ id: "i", machineId: "atlas", machineLabel: "Atlas", session: "s", kind: NOTICE_KIND, createdAt: "2026-10-01T19:35:00Z" }, plan.content);
    expect(attentionContent(item)).toMatchObject({ headline: "The supervisor missed an update: a worker stopped", severity: "warning", action: "view_pane", fingerprint: "notice:atlas:s:7" });
  });
});

describe("notice age (cas-5c22)", () => {
  const now = new Date(2026, 9, 1, 11, 0).getTime();
  it("keeps a replayed notice's own time, and a live one has none", () => {
    expect(noticeTime("2026-09-30T17:49:00Z", now)).toBe("2026-09-30T17:49:00.000Z");
    expect(noticeTime("2026-09-30T19:49:00+02:00", now)).toBe("2026-09-30T17:49:00.000Z");
    expect(noticeTime(undefined, now)).toBeUndefined();
    expect(noticeTime("not a time", now)).toBeUndefined();
    // A machine clock ahead of this browser never dates it in the future.
    expect(noticeTime(new Date(now + 600_000).toISOString(), now)).toBe(new Date(now).toISOString());
  });
  it("shows an earlier day's notice by its date, today's by its age", () => {
    expect(attentionTimeLabel(new Date(2026, 8, 30, 17, 49).toISOString(), now)).toBe("Sep 30, 17:49");
    expect(attentionTimeLabel(new Date(2025, 8, 30, 17, 49).toISOString(), now)).toBe("Sep 30 2025, 17:49");
    expect(attentionTimeLabel(new Date(now - 23 * 60_000).toISOString(), now)).toBe("23m");
    expect(attentionTimeLabel(new Date(now - 20_000).toISOString(), now)).toBe("now");
  });
  it("names the phone badge by its count and hides it at zero", () => {
    expect(conversationAttentionBadge(0)).toMatchObject({ hidden: true });
    expect(conversationAttentionBadge(1)).toEqual({ hidden: false, text: "1", label: "Attention: 1 item for this session" });
    expect(conversationAttentionBadge(3).label).toBe("Attention: 3 items for this session");
  });
});

// cas-7cb3: a watchdog's relative age is a historical diagnostic, not current copy.
describe("delivery notice copy", () => {
  it("keeps a stale age only in Details, and names the stopped worker plainly", () => {
    const raw = watchdog({ source: "relay-watchdog", subject: 3196290 });
    const plan = planNotice("atlas", "Accounting-rapid-gazelle-52", raw, () => false);
    if (plan.action !== "raise") throw new Error("expected raise");
    expect(plan.content.headline).toBe("The supervisor missed an update: a worker stopped");
    expect(plan.content.detail).toBe("Worker daring-robin-43 stopped. The update did not reach the supervisor.");
    expect(JSON.stringify(plan.content.payload)).toContain("9 minutes ago");
    const legacy = createAttentionItem({ id: "old", machineId: "atlas", machineLabel: "Atlas", session: "s", kind: NOTICE_KIND, createdAt: "2026-09-30T17:49:00Z" }, { headline: raw.summary!, detail: raw.message, severity: "warning", action: "view_pane" });
    expect(attentionContent(legacy).headline).toBe(plan.content.headline);
    expect(attentionContent(legacy).detail).not.toMatch(/9m|9 minutes ago|worker died/);
  });
});
