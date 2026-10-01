/**
 * System notices (cas-e829). A relay-watchdog alert ("the supervisor never saw
 * an update") is about a session's plumbing, not something its supervisor
 * said. It belongs in the attention lane: one item per unseen relay, however
 * often it is replayed, retired once the relay reaches the supervisor or the
 * session ends. It is never a conversation turn.
 */
import type { AttentionContent } from "./attention";
import type { OperatorReply } from "./types";

/** The attention kind a notice raises (attention.ts MACHINE_EVENT_TEMPLATES). */
export const NOTICE_KIND = "delivery_stall";

/** Whether a supervisor-lane row is a system notice rather than a turn. */
export function isOperatorNotice(reply: Pick<OperatorReply, "notice">): boolean {
  return reply.notice !== undefined && reply.notice !== null;
}

/** Every notice of one session shares this prefix, so a session that ends retires them together. */
export function noticeSessionPrefix(machineId: string, session: string): string {
  return `notice:${machineId}:${session}:`;
}

/**
 * One item per thing the notice is about: the unseen relay when it names one,
 * else the notice row itself. Replays and repeats land on the same item.
 */
export function noticeFingerprint(machineId: string, session: string, notificationId: number, subject?: number): string {
  return `${noticeSessionPrefix(machineId, session)}${subject ?? notificationId}`;
}

export type NoticePlan =
  | { action: "raise"; fingerprint: string; content: AttentionContent }
  | { action: "resolve"; fingerprint: string }
  | { action: "none"; fingerprint: string };

/**
 * What a notice does to the attention lane. `known` says whether an item with
 * this fingerprint was ever raised here, open or dismissed: a replayed notice
 * never raises a second item, and a dismissed one stays dismissed.
 */
export function planNotice(machineId: string, session: string, reply: OperatorReply, known: (fingerprint: string) => boolean): NoticePlan {
  const notice = reply.notice!;
  const fingerprint = noticeFingerprint(machineId, session, reply.notification_id, notice.subject);
  if (notice.resolved) return { action: "resolve", fingerprint };
  if (known(fingerprint)) return { action: "none", fingerprint };
  return {
    action: "raise",
    fingerprint,
    content: {
      headline: reply.summary?.trim() || "The supervisor missed an update",
      detail: reply.message,
      severity: "warning",
      action: "view_pane",
      fingerprint,
    },
  };
}
