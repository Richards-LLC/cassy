// Project stored inbox events into Commander's conversation model (cas-9b7d S3).
//
// A hub-produced event's plaintext carries the m263 frozen snapshot of one
// prompt-queue row. Its `prompt_id` is the same durable notification id the
// hub's own history page uses, so a turn that arrives both directly and from
// the cloud is one bubble: Commander already merges history rows by
// `(machine, notification_id)`. Identity is never text or timestamp.
//
// A device-produced command history event has no prompt id yet (the machine
// may still be off); it projects as a queued operator message keyed by its
// command ID.
//
// Observer notices are not conversation turns. They are stored, verified,
// and left to the presence UI (cas-e3dd).

import type { InboxEvent } from "./store";

export interface InboxMessage {
  kind: "message";
  hubId: string;
  /** Routing IDs of the conversation, for read marks and offline replies. */
  projectId: string;
  sessionId: string;
  session: string;
  notificationId: number;
  text: string;
  at: string;
  deviceId: string;
  operatorLabel?: string;
  replyTo?: number;
  eventId: string;
  sequence: string;
}

export interface InboxReply {
  kind: "reply";
  hubId: string;
  /** Routing IDs of the conversation, for read marks and offline replies. */
  projectId: string;
  sessionId: string;
  session: string;
  notificationId: number;
  message: string;
  summary: string;
  at: string;
  deviceId: string;
  replyTo: number | null;
  turnKind?: string;
  attachments: unknown[];
  eventId: string;
  sequence: string;
}

export interface InboxQueuedCommand {
  kind: "command";
  hubId: string;
  /** Routing IDs of the conversation, for read marks and offline replies. */
  projectId: string;
  sessionId: string;
  session: string;
  commandId: string;
  text: string;
  at: string;
  deviceId: string;
  eventId: string;
  sequence: string;
}

export type InboxTurn = InboxMessage | InboxReply | InboxQueuedCommand;

const TURN_KINDS = new Set(["answer", "status", "receipt", "ask", "blocker"]);

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function text(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function positiveInt(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : undefined;
}

/** One verified session event as a conversation turn, or null when it is not one. */
export function projectInboxEvent(event: InboxEvent): InboxTurn | null {
  if (event.verification !== "verified" || event.scope !== "session") return null;
  const plain = asRecord(event.plaintext);
  if (!plain || plain.type !== "cas.operator.turn" || plain.v !== 1) return null;
  const snapshot = asRecord(plain.snapshot);
  if (!snapshot) return null;
  const session = text(snapshot.factory_session) ?? text(plain.session_name);
  const at = text(snapshot.created_at) ?? event.storedAt;
  if (!session) return null;
  if (!event.projectId || !event.sessionId) return null;
  const base = { hubId: event.hubId, projectId: event.projectId, sessionId: event.sessionId, session, at, eventId: event.eventId, sequence: event.sequence };

  const commandId = text(snapshot.command_id);
  const promptId = positiveInt(snapshot.prompt_id);
  if (commandId && promptId === undefined) {
    const source = text(snapshot.source) ?? "";
    return { kind: "command", ...base, commandId, text: text(snapshot.prompt) ?? "", deviceId: source.replace(/^commander:/, "") };
  }
  if (promptId === undefined) return null;

  const target = (text(snapshot.target) ?? "").trim().toLowerCase();
  const operator = asRecord(snapshot.operator);
  if (target === "operator") {
    const kind = text(snapshot.kind);
    return {
      kind: "reply",
      ...base,
      notificationId: promptId,
      message: text(snapshot.prompt) ?? "",
      summary: text(snapshot.summary) ?? "",
      deviceId: text(snapshot.recipient_device_id) ?? "",
      replyTo: positiveInt(snapshot.acknowledge_prompt_id) ?? null,
      turnKind: kind && TURN_KINDS.has(kind) ? kind : undefined,
      attachments: Array.isArray(snapshot.attachments) ? snapshot.attachments : [],
    };
  }
  if (operator) {
    return {
      kind: "message",
      ...base,
      notificationId: promptId,
      text: text(snapshot.prompt) ?? "",
      deviceId: text(operator.device_id) ?? "",
      operatorLabel: text(operator.operator),
    };
  }
  return null;
}

/** All turns of one machine, grouped by session, in feed order. */
export function inboxThreads(events: readonly InboxEvent[], hubId: string): Map<string, InboxTurn[]> {
  const threads = new Map<string, InboxTurn[]>();
  const seen = new Set<string>();
  const ordered = [...events].sort((a, b) => (BigInt(a.sequence) < BigInt(b.sequence) ? -1 : 1));
  for (const event of ordered) {
    if (event.hubId !== hubId) continue;
    const turn = projectInboxEvent(event);
    if (!turn) continue;
    const identity = turn.kind === "command" ? `c:${turn.commandId}` : `${turn.kind}:${turn.notificationId}`;
    if (seen.has(identity)) continue;
    seen.add(identity);
    const list = threads.get(turn.session) ?? [];
    list.push(turn);
    threads.set(turn.session, list);
  }
  return threads;
}
