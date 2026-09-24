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
/** `clockAhead` marks a live supervisor turn from a machine whose clock this
 * thread has seen running ahead: its time is the arrival time, as a reloaded
 * copy of it would show, and says so the same way (cas-1f13). */
/** `arrivedAt` is when this browser received the turn (ms epoch): a live turn
 * when it came off the socket, a durable one when its history page was
 * hydrated. It serves two readers: the receipt grace for an earlier send
 * counts from a later reply's arrival (cas-1185), and a machine stamp later
 * than the arrival shows the arrival instead, so it cannot drift with the
 * render clock (cas-1f13). */
export type ConversationEvent = { kind: "send"; value: ConversationSend; at?: number; shownAt?: number; arrivedAt?: number; clockAhead?: boolean; session?: string } | { kind: "reply"; value: OperatorReply; at?: number; shownAt?: number; arrivedAt?: number; clockAhead?: boolean; session?: string };

/**
 * The machine's durable sequence for a turn: its prompt-queue row id. Operator
 * messages and supervisor replies share that one sequence (daemon
 * `conversation_history_page`), so it is the order the machine received them,
 * whatever its clock says (cas-1f13).
 */
export function durableSeq(event: ConversationEvent): number | undefined {
  return event.kind === "send" ? event.value.notificationId : event.value.notification_id;
}

/** A machine stamp this far past the browser's clock means the machine's clock runs ahead. */
const CLOCK_AHEAD_MS = 60_000;

/** In-memory per-thread evidence. A submitted socket frame is never a receipt. */
export class ConversationHistory {
  readonly events: ConversationEvent[] = [];
  /** A durable stamp from this thread's machine has been seen in this browser's future. */
  private machineAhead = false;
  private insert(event: ConversationEvent): void {
    const at = event.at ?? Number.POSITIVE_INFINITY;
    const index = this.events.findIndex((existing) => (existing.at ?? Number.POSITIVE_INFINITY) > at);
    if (index < 0) this.events.push(event);
    else this.events.splice(index, 0, event);
  }

  /**
   * Place a durable turn by the machine's sequence, not its clock (cas-1f13):
   * before the first turn the machine sequenced after it. With none, it goes
   * after the last turn sequenced before it and then by time among the live
   * turns that follow. A reload therefore rebuilds the same order a visit saw,
   * even when the machine's clock runs ahead.
   */
  private insertDurable(event: ConversationEvent): void {
    const seq = durableSeq(event);
    if (seq === undefined) { this.insert(event); return; }
    const later = this.events.findIndex((existing) => { const other = durableSeq(existing); return other !== undefined && other > seq; });
    if (later >= 0) { this.events.splice(later, 0, event); return; }
    let start = 0;
    this.events.forEach((existing, index) => { const other = durableSeq(existing); if (other !== undefined && other < seq) start = index + 1; });
    const at = event.at ?? Number.POSITIVE_INFINITY;
    const index = this.events.findIndex((existing, position) => position >= start && (existing.at ?? Number.POSITIVE_INFINITY) > at);
    if (index < 0) this.events.push(event);
    else this.events.splice(index, 0, event);
  }

  /** Note a machine stamp ahead of this browser: later live turns say the clock is ahead. */
  private observeStamp(at: number | undefined, now: number): void {
    if (at !== undefined && at - now >= CLOCK_AHEAD_MS) this.machineAhead = true;
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
  hydrateSend(message: ConversationHistoryMessage, now: number = Date.now()): void {
    const existing = this.events.find((event) => event.kind === "send" && event.value.notificationId === message.notification_id);
    if (existing?.kind === "send") {
      existing.value.target = message.target;
      existing.value.text = message.text;
      existing.value.state = message.state;
      existing.value.stamped = message.stamped;
      // A turn already in the thread keeps what it is known to be. A
      // reconnect's history row that omits in_reply_to or the session must
      // not re-open the ask it answered or move it across the session line
      // (cas-1f13); in_reply_to never changes after the send.
      existing.value.replyTo = message.reply_to ?? existing.value.replyTo;
      existing.session ??= message.session;
      return;
    }
    const at = ConversationHistory.timestamp(message.at);
    this.observeStamp(at, now);
    this.insertDurable({
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
      at,
      arrivedAt: now,
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
  reply(reply: OperatorReply, at: number | undefined = Date.now(), session?: string, shownAt?: number, arrivedAt: number = Date.now(), placement: "time" | "durable" = "time", clockAhead = false): void {
    if (this.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) return;
    const normalized: OperatorReply = {
      ...reply,
      reply_to: reply.reply_to ?? null,
      kind: reply.kind ?? "answer",
      attachments: reply.attachments ?? [],
    };
    const event: ConversationEvent = { kind: "reply", value: normalized, at, ...(shownAt === undefined ? {} : { shownAt }), arrivedAt, ...(clockAhead ? { clockAhead } : {}), session };
    if (placement === "durable") this.insertDurable(event);
    else this.insert(event);
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
    this.reply(reply, key, session, key === at ? undefined : at, at, "time", this.machineAhead);
  }

  /** Merge a durable supervisor turn in the machine's sequence, keeping its own stamp (cas-1f13). */
  hydrateReply(reply: ConversationHistoryReply, now: number = Date.now()): void {
    const { at, ...live } = reply;
    const stamped = ConversationHistory.timestamp(at);
    this.observeStamp(stamped, now);
    this.reply(live, stamped, reply.session, undefined, now, "durable");
  }
}
