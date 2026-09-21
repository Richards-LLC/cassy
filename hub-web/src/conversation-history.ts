import type { ConversationHistoryMessage, ConversationHistoryReply, MessageQueued, OperatorReply } from "./types";

export interface ConversationSend {
  id: string;
  target: string;
  text: string;
  state: "sending" | "acknowledged" | "replied" | "error";
  notificationId?: number;
  stamped?: boolean;
  error?: string;
  /** notification_id of the supervisor ask this send answers (in_reply_to on the wire). */
  replyTo?: number;
}
/** `at` is when this client saw the event (ms epoch); it stamps the thread's
 * day separators and group timestamps and is never a delivery receipt. */
export type ConversationEvent = { kind: "send"; value: ConversationSend; at?: number } | { kind: "reply"; value: OperatorReply; at?: number };

/** In-memory per-thread evidence. A submitted socket frame is never a receipt. */
export class ConversationHistory {
  readonly events: ConversationEvent[] = [];
  private insert(event: ConversationEvent): void {
    const at = event.at ?? Number.POSITIVE_INFINITY;
    const index = this.events.findIndex((existing) => (existing.at ?? Number.POSITIVE_INFINITY) > at);
    if (index < 0) this.events.push(event);
    else this.events.splice(index, 0, event);
  }

  private static timestamp(value: string | undefined): number | undefined {
    if (!value) return undefined;
    const parsed = Date.parse(value);
    return Number.isFinite(parsed) ? parsed : undefined;
  }

  hasPending(): boolean {
    return this.events.some(event => event.kind === "send" && (event.value.state === "sending" || event.value.state === "acknowledged"));
  }
  submit(id: string, target: string, text: string, at: number = Date.now(), replyTo?: number): void {
    this.insert({ kind: "send", value: { id, target, text, state: "sending", ...(replyTo === undefined ? {} : { replyTo }) }, at });
  }

  /** Merge one durable operator message without duplicating a live ack. */
  hydrateSend(message: ConversationHistoryMessage): void {
    const existing = this.events.find((event) => event.kind === "send" && event.value.notificationId === message.notification_id);
    if (existing?.kind === "send") {
      existing.value.target = message.target;
      existing.value.text = message.text;
      existing.value.state = message.state;
      existing.value.stamped = message.stamped;
      existing.value.replyTo = message.reply_to;
      return;
    }
    this.insert({
      kind: "send",
      value: {
        id: `history:${message.notification_id}`,
        target: message.target,
        text: message.text,
        state: message.state,
        notificationId: message.notification_id,
        stamped: message.stamped,
        ...(message.reply_to === undefined ? {} : { replyTo: message.reply_to }),
      },
      at: ConversationHistory.timestamp(message.at),
    });
  }
  /** The operator send that answered this ask, if any. */
  answered(notificationId: number): ConversationSend | undefined {
    for (const event of this.events) if (event.kind === "send" && event.value.replyTo === notificationId) return event.value;
    return undefined;
  }
  /**
   * Asks and blockers still waiting on the operator, oldest first. An ask is
   * answered by a send carrying its id; a blocker is acknowledged by any
   * operator send after it. Drives the list's waiting affordance and the pin.
   */
  waiting(): OperatorReply[] {
    const out: OperatorReply[] = [];
    this.events.forEach((event, index) => {
      if (event.kind !== "reply") return;
      const reply = event.value;
      if (reply.kind === "ask" && !this.answered(reply.notification_id)) out.push(reply);
      else if (reply.kind === "blocker" && !this.events.slice(index + 1).some((later) => later.kind === "send")) out.push(reply);
    });
    return out;
  }
  /** The ask pinned above the composer: the most recent unanswered one. */
  pinnedAsk(): OperatorReply | undefined {
    return this.waiting().filter((reply) => reply.kind === "ask").at(-1);
  }
  acknowledge(receipt: MessageQueued): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === receipt.client_ref && event.value.target === receipt.target)
      ?? this.events.find((event) => event.kind === "send" && event.value.notificationId === receipt.notification_id && event.value.target === receipt.target);
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
  reply(reply: OperatorReply, at: number | undefined = Date.now()): void {
    if (this.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) return;
    const normalized: OperatorReply = {
      ...reply,
      reply_to: reply.reply_to ?? null,
      kind: reply.kind ?? "answer",
      attachments: reply.attachments ?? [],
    };
    this.insert({ kind: "reply", value: normalized, at });
    for (const event of this.events) {
      if (event.kind === "send" && normalized.reply_to !== null && event.value.notificationId === normalized.reply_to) event.value.state = "replied";
    }
  }

  /** Merge a durable supervisor turn using its original queue timestamp. */
  hydrateReply(reply: ConversationHistoryReply): void {
    const { at, ...live } = reply;
    this.reply(live, ConversationHistory.timestamp(at));
  }
}
