import type { MessageQueued, OperatorReply } from "./types";

export interface ConversationSend {
  id: string;
  target: string;
  text: string;
  state: "sending" | "acknowledged" | "replied" | "error";
  notificationId?: number;
  stamped?: boolean;
  error?: string;
}
/** `at` is when this client saw the event (ms epoch); it stamps the thread's
 * day separators and group timestamps and is never a delivery receipt. */
export type ConversationEvent = { kind: "send"; value: ConversationSend; at?: number } | { kind: "reply"; value: OperatorReply; at?: number };

/** In-memory per-thread evidence. A submitted socket frame is never a receipt. */
export class ConversationHistory {
  readonly events: ConversationEvent[] = [];
  hasPending(): boolean {
    return this.events.some(event => event.kind === "send" && (event.value.state === "sending" || event.value.state === "acknowledged"));
  }
  submit(id: string, target: string, text: string, at: number = Date.now()): void {
    this.events.push({ kind: "send", value: { id, target, text, state: "sending" }, at });
  }
  acknowledge(receipt: MessageQueued): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === receipt.client_ref && event.value.target === receipt.target);
    if (!send || send.kind !== "send") return false;
    send.value.notificationId = receipt.notification_id;
    send.value.stamped = receipt.stamped;
    send.value.state = this.events.some((event) => event.kind === "reply" && event.value.reply_to === receipt.notification_id) ? "replied" : "acknowledged";
    delete send.value.error;
    return true;
  }
  reject(id: string, message: string): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === id);
    if (!send || send.kind !== "send" || send.value.notificationId !== undefined) return false;
    send.value.state = "error";
    send.value.error = message;
    return true;
  }
  reply(reply: OperatorReply, at: number = Date.now()): void {
    if (this.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) return;
    const normalized: OperatorReply = {
      ...reply,
      reply_to: reply.reply_to ?? null,
      kind: reply.kind ?? "answer",
      attachments: reply.attachments ?? [],
    };
    this.events.push({ kind: "reply", value: normalized, at });
    for (const event of this.events) {
      if (event.kind === "send" && normalized.reply_to !== null && event.value.notificationId === normalized.reply_to) event.value.state = "replied";
    }
  }
}
