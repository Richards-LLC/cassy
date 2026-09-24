import type { ConversationHistoryMessage, ConversationHistoryReply, MessageQueued, OperatorReply } from "./types";

/**
 * How long a live send waits for the hub's delivery receipt (MessageQueued)
 * before it stops saying "Sending…" and offers Retry (cas-1622). The hub
 * answers a send at once, so a receipt this late is not coming.
 */
export const RECEIPT_TIMEOUT_MS = 15_000;
/**
 * A supervisor turn that lands after a send is later traffic on the same
 * socket; the send's receipt would have come first. It still gets this long,
 * counted from when that turn arrived here, so a reply racing the receipt
 * never flashes "Not confirmed" (cas-1622). Counted from the send, a 2 s grace
 * turned a receipt 1.9–3.4 s late into a brief "Not confirmed" with Retry, and
 * a Retry tapped then sent the message twice (cas-1185).
 */
export const RECEIPT_REPLY_GRACE_MS = 5_000;

export interface ConversationSend {
  id: string;
  target: string;
  text: string;
  /** `unconfirmed`: sent, but the hub's receipt never came (cas-1622). It may
   * or may not have arrived; the operator can retry it. */
  state: "sending" | "acknowledged" | "replied" | "error" | "unconfirmed";
  /** When this browser put the send on the wire (ms epoch); live sends only. */
  sentAt?: number;
  notificationId?: number;
  stamped?: boolean;
  error?: string;
  /** notification_id of the supervisor ask this send answers (in_reply_to on the wire). */
  replyTo?: number;
  /** A refused send whose edited version has since gone out: it stays in the
   * thread as a record, but offers nothing to retry. */
  replaced?: boolean;
}
/** `at` is when this client saw the event (ms epoch); it stamps the thread's
 * day separators and group timestamps and is never a delivery receipt. */
/** `at` is the sort key. `shownAt`, when set, is the time to display: a live
 * event sorted after a turn stamped by a clock that runs ahead keeps its own
 * time on screen (cas-ac1f). */
/** A reply's `arrivedAt` is when this browser received it (ms epoch): the
 * receipt grace for an earlier send counts from it (cas-1185). */
export type ConversationEvent = { kind: "send"; value: ConversationSend; at?: number; shownAt?: number; session?: string } | { kind: "reply"; value: OperatorReply; at?: number; shownAt?: number; session?: string; arrivedAt?: number };

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
  /**
   * A new send comes after everything already in the thread, whatever the
   * clocks say (cas-ce17). Turns hydrated from the machine carry its clock;
   * when that runs ahead of this browser's, a send stamped with the browser
   * clock would sort before the blocker or ask it answers, leaving it
   * "waiting". The send is placed at or after the latest turn instead.
   */
  submit(id: string, target: string, text: string, at: number = Date.now(), replyTo?: number, session?: string): void {
    const key = Math.max(at, this.latestAt());
    this.insert({ kind: "send", value: { id, target, text, state: "sending", sentAt: at, ...(replyTo === undefined ? {} : { replyTo }) }, at: key, ...(key === at ? {} : { shownAt: at }), session });
  }

  /** The latest stamp already in the thread; live events are placed at or after it. */
  private latestAt(): number {
    return this.events.reduce((max, event) => (event.at !== undefined && Number.isFinite(event.at) && event.at > max ? event.at : max), Number.NEGATIVE_INFINITY);
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
      existing.session = message.session;
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
      session: message.session,
    });
  }
  /**
   * The operator send that answered this ask, if any. A refused send never
   * reached the supervisor, so it answers nothing: the ask stays waiting (and
   * pinned, with its chips) beside the refused bubble until a send that is
   * sending, acknowledged or replied carries its id. A sending reply answers
   * optimistically and gives the ask back if the hub refuses it.
   */
  answered(notificationId: number): ConversationSend | undefined {
    for (const event of this.events) if (event.kind === "send" && event.value.replyTo === notificationId && event.value.state !== "error") return event.value;
    return undefined;
  }
  /**
   * Asks and blockers still waiting on the operator, oldest first. An ask is
   * answered by a send carrying its id; a blocker is acknowledged by any
   * operator send after it that was not refused. A refused send never reached
   * the supervisor, so the blocker keeps waiting beside it; a sending send
   * acknowledges optimistically and gives the blocker back if refused, and a
   * successful retry acknowledges it. Drives the list's waiting affordance and
   * the pin.
   */
  waiting(): OperatorReply[] {
    const out: OperatorReply[] = [];
    this.events.forEach((event, index) => {
      if (event.kind !== "reply") return;
      const reply = event.value;
      if (reply.kind === "ask" && !this.answered(reply.notification_id)) out.push(reply);
      else if (reply.kind === "blocker" && !this.events.slice(index + 1).some((later) => later.kind === "send" && later.value.state !== "error")) out.push(reply);
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
  /**
   * Live sends still waiting on a receipt past their deadline become
   * `unconfirmed`: RECEIPT_TIMEOUT_MS after they went out, or
   * RECEIPT_REPLY_GRACE_MS after a later supervisor turn arrived here,
   * whichever comes first (cas-1622, cas-1185). Returns the ids that changed. A late receipt still turns one
   * into "Delivered" (acknowledge), and a late refusal into "Not sent".
   */
  unconfirmSilent(now: number): string[] {
    const changed: string[] = [];
    this.events.forEach((event, index) => {
      const deadline = this.receiptDeadline(index);
      if (deadline === undefined || now < deadline || event.kind !== "send") return;
      event.value.state = "unconfirmed";
      changed.push(event.value.id);
    });
    return changed;
  }
  /** Milliseconds until the next send could become unconfirmed, if any is waiting. */
  nextReceiptCheck(now: number): number | undefined {
    let next: number | undefined;
    this.events.forEach((_, index) => {
      const deadline = this.receiptDeadline(index);
      if (deadline !== undefined && (next === undefined || deadline < next)) next = deadline;
    });
    return next === undefined ? undefined : Math.max(0, next - now);
  }
  private receiptDeadline(index: number): number | undefined {
    const event = this.events[index];
    if (event?.kind !== "send" || event.value.state !== "sending" || event.value.notificationId !== undefined || event.value.sentAt === undefined) return undefined;
    const sentAt = event.value.sentAt;
    const timeout = sentAt + RECEIPT_TIMEOUT_MS;
    // The grace starts when the later turn reached this browser, not at the
    // send: a turn that crosses the send must not shorten the receipt's wait.
    const arrivals = this.events.slice(index + 1).flatMap((later) => later.kind === "reply" ? [later.arrivedAt ?? sentAt] : []);
    if (!arrivals.length) return timeout;
    return Math.min(timeout, Math.max(sentAt, Math.min(...arrivals)) + RECEIPT_REPLY_GRACE_MS);
  }
  /**
   * Drop a refused or unconfirmed send that a retry replaced. Only those can
   * be discarded: a delivered send is never taken back.
   */
  discardRefused(id: string): boolean {
    const index = this.events.findIndex((event) => event.kind === "send" && event.value.id === id && (event.value.state === "error" || event.value.state === "unconfirmed"));
    if (index < 0) return false;
    this.events.splice(index, 1);
    return true;
  }
  /**
   * Retire a refused send whose edited version is now on the wire. It stays in
   * the thread, collapsed, so the log still shows what was refused; it can no
   * longer be retried, which would resend the text the operator just corrected.
   */
  retireRefused(id: string): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === id && event.value.state === "error");
    if (!send || send.kind !== "send") return false;
    send.value.replaced = true;
    return true;
  }
  /**
   * The operator's latest delivered send while its answer is still to come:
   * the hub queued it (a MessageQueued receipt, or a durable history row) and
   * no supervisor turn has landed since. Sends still in flight or refused after
   * it do not hide it — it is still the latest message known to have arrived.
   */
  delivered(): ConversationSend | undefined {
    for (let index = this.events.length - 1; index >= 0; index -= 1) {
      const event = this.events[index]!;
      if (event.kind === "reply") return undefined;
      if (event.value.state === "replied") return undefined;
      if (event.value.state === "acknowledged") return event.value;
    }
    return undefined;
  }
  /**
   * The conversation list's one-line preview of the last turn. A refused send
   * was never said, so it reads as not sent rather than "You: …"; a refused
   * send an edit replaced is skipped, since its edit is what was said.
   */
  preview(): string | undefined {
    for (let index = this.events.length - 1; index >= 0; index -= 1) {
      const event = this.events[index]!;
      if (event.kind === "reply") return event.value.message;
      if (event.value.state !== "error") return `You: ${event.value.text}`;
      if (!event.value.replaced) return `Not sent: ${event.value.text}`;
    }
    return undefined;
  }
  reply(reply: OperatorReply, at: number | undefined = Date.now(), session?: string, shownAt?: number, arrivedAt: number = Date.now()): void {
    if (this.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) return;
    const normalized: OperatorReply = {
      ...reply,
      reply_to: reply.reply_to ?? null,
      kind: reply.kind ?? "answer",
      attachments: reply.attachments ?? [],
    };
    this.insert({ kind: "reply", value: normalized, at, ...(shownAt === undefined ? {} : { shownAt }), session, arrivedAt });
    for (const event of this.events) {
      if (event.kind === "send" && normalized.reply_to !== null && event.value.notificationId === normalized.reply_to) event.value.state = "replied";
    }
  }

  /**
   * A supervisor turn arriving live happens after everything already shown,
   * like a send (cas-ce17): with a machine clock ahead of this browser's, a
   * browser-stamped turn would otherwise sort before the operator's last
   * answer and read as already acknowledged.
   */
  receive(reply: OperatorReply, at: number = Date.now(), session?: string): void {
    const key = Math.max(at, this.latestAt());
    this.reply(reply, key, session, key === at ? undefined : at, at);
  }

  /** Merge a durable supervisor turn using its original queue timestamp. */
  hydrateReply(reply: ConversationHistoryReply): void {
    const { at, ...live } = reply;
    this.reply(live, ConversationHistory.timestamp(at), reply.session);
  }
}
