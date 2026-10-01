import { describe, expect, it } from "vitest";
import { attentionContent, createAttentionItem } from "./attention";
import { isOperatorNotice, NOTICE_KIND, noticeFingerprint, planNotice } from "./operator-notices";
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
    expect(first.content).toMatchObject({ headline: "Supervisor hasn't seen: worker died: daring-robin-43 (9m)", severity: "warning", action: "view_pane" });
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
    expect(attentionContent(item)).toMatchObject({ headline: "Supervisor hasn't seen: worker died: daring-robin-43 (9m)", severity: "warning", action: "view_pane", fingerprint: "notice:atlas:s:7" });
  });
});
