import type { OperatorReply } from "./types";

export interface MessageReceipt {
  client_ref: string;
  notification_id: number;
  target: string;
  stamped: boolean;
}
export interface ConversationSend {
  id: string;
  target: string;
  text: string;
  state: "sending" | "acknowledged" | "replied" | "error";
  notificationId?: number;
  stamped?: boolean;
  error?: string;
}
export type ConversationEvent = { kind: "send"; value: ConversationSend } | { kind: "reply"; value: OperatorReply };

/** In-memory per-thread evidence. A submitted socket frame is never a receipt. */
export class ConversationHistory {
  readonly events: ConversationEvent[] = [];
  submit(id: string, target: string, text: string): void {
    this.events.push({ kind: "send", value: { id, target, text, state: "sending" } });
  }
  acknowledge(receipt: MessageReceipt): boolean {
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
  reply(reply: OperatorReply): void {
    if (this.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) return;
    this.events.push({ kind: "reply", value: reply });
    for (const event of this.events) {
      if (event.kind === "send" && event.value.notificationId === reply.reply_to) event.value.state = "replied";
    }
  }
}
