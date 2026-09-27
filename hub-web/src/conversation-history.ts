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
  /** When this browser gave up on the receipt (ms epoch; `unconfirmed` only). */
  unconfirmedAt?: number;
  /** The operator swiped or dismissed this failed send out of the thread
   * (cas-16eed). It is kept, not deleted: the "unsent" chip brings it back. */
  dismissed?: boolean;
  /** Held in this browser while the machine is unreachable: never on the
   * wire yet, so it goes out once (with its own client_ref) when the
   * session is back, or turns "Not sent" if it is not back in time (cas-0978). */
  held?: boolean;
}

/**
 * Why an unanswered ask (or blocker) no longer waits on the operator
 * (cas-16eed): the operator dismissed it, or the supervisor session that asked
 * has ended. Nothing the supervisor says in the meantime retires it: a
 * supervisor posts progress while it waits, and a question it is still
 * waiting on keeps its pin and its choices (cas-16eed QA F01).
 */
export type AskRetirement = "dismissed" | "session-ended";
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
  /**
   * The supervisor session this thread is attached to now. A question stamped
   * with another session was asked by one that has since ended (cas-16eed).
   */
  currentSession?: string;
  /** Asks and blockers the operator dismissed (notification ids). */
  private readonly dismissedAsks = new Set<number>();
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

  /**
   * A send the page could not put on the wire because the machine is
   * unreachable. It shows in the thread as waiting and has no receipt
   * deadline until `release` puts it on the wire.
   */
  hold(id: string, target: string, text: string, at: number = Date.now(), replyTo?: number, session?: string): void {
    const key = Math.max(at, this.latestAt());
    this.insert({ kind: "send", value: { id, target, text, state: "sending", held: true, ...(replyTo === undefined ? {} : { replyTo }) }, at: key, ...(key === at ? {} : { shownAt: at }), session });
  }

  /** The held send went out now: its receipt clock starts. */
  release(id: string, at: number = Date.now()): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === id);
    if (!send || send.kind !== "send" || !send.value.held) return false;
    delete send.value.held;
    send.value.sentAt = at;
    return true;
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
   * the supervisor, so the blocker keeps waiting beside it, and so does one
   * whose receipt never came (Not confirmed, cas-71af); a sending send
   * acknowledges optimistically and gives the blocker back if refused, and a
   * successful retry acknowledges it. Drives the list's waiting affordance and
   * the pin.
   */
  waiting(): OperatorReply[] {
    const out: OperatorReply[] = [];
    this.events.forEach((event, index) => {
      if (event.kind !== "reply") return;
      const reply = event.value;
      if ((reply.kind === "ask" || reply.kind === "blocker") && this.retirementAt(index)) return;
      if (reply.kind === "ask" && !this.answered(reply.notification_id)) out.push(reply);
      else if (reply.kind === "blocker" && !this.events.slice(index + 1).some((later) => later.kind === "send" && later.value.state !== "error" && later.value.state !== "unconfirmed")) out.push(reply);
    });
    return out;
  }
  /** The ask pinned above the composer: the most recent unanswered one. */
  pinnedAsk(): OperatorReply | undefined {
    return this.waiting().filter((reply) => reply.kind === "ask").at(-1);
  }
  /**
   * Why an ask or blocker that was never answered no longer waits (cas-16eed),
   * or undefined while it still does. An answered ask is the answer's story,
   * not a retirement.
   */
  retirement(notificationId: number): AskRetirement | undefined {
    const index = this.events.findIndex((event) => event.kind === "reply" && event.value.notification_id === notificationId);
    return index < 0 ? undefined : this.retirementAt(index);
  }
  private retirementAt(index: number): AskRetirement | undefined {
    const event = this.events[index];
    if (event?.kind !== "reply" || (event.value.kind !== "ask" && event.value.kind !== "blocker")) return undefined;
    const reply = event.value;
    if (reply.kind === "ask" && this.answered(reply.notification_id)) return undefined;
    if (this.dismissedAsks.has(reply.notification_id)) return "dismissed";
    const later = this.events.slice(index + 1);
    // Asked by a session that has ended: the thread is attached to another
    // one now, or a later turn already came from another one.
    if (event.session && ((this.currentSession !== undefined && event.session !== this.currentSession) || later.some((next) => next.session !== undefined && next.session !== event.session))) return "session-ended";
    return undefined;
  }
  /** The operator dismissed a waiting ask or blocker: it unpins and stops waiting. */
  dismissAsk(notificationId: number): boolean {
    if (this.dismissedAsks.has(notificationId)) return false;
    this.dismissedAsks.add(notificationId);
    return true;
  }
  /** Notification ids of the asks and blockers the operator dismissed. */
  dismissedAskIds(): number[] { return [...this.dismissedAsks]; }
  /**
   * Hide a failed send (refused, or unconfirmed and not yet settled) from the
   * thread (cas-16eed). Its Edit and Retry come back with it on restore.
   */
  dismissSend(id: string): boolean {
    const event = this.events.find((candidate) => candidate.kind === "send" && candidate.value.id === id);
    if (event?.kind !== "send" || event.value.dismissed || !this.isFailedSend(event.value)) return false;
    event.value.dismissed = true;
    return true;
  }
  /** Failed sends the operator dismissed and can still bring back. */
  dismissedSends(): ConversationSend[] {
    return this.events.flatMap((event) => event.kind === "send" && event.value.dismissed && this.isFailedSend(event.value) ? [event.value] : []);
  }
  /** Bring every dismissed failed send back into the thread; returns them. */
  restoreDismissed(): ConversationSend[] {
    const restored = this.dismissedSends();
    for (const send of restored) delete send.dismissed;
    return restored;
  }
  /** A send that did not go and still offers a way to send it: refused, or unconfirmed with no reply since. */
  isFailedSend(send: ConversationSend): boolean {
    return (send.state === "error" && !send.replaced) || (send.state === "unconfirmed" && !this.repliedSince(send));
  }
  /** Events as the thread shows them: without the failed sends the operator dismissed. */
  visibleEvents(): ConversationEvent[] {
    return this.events.filter((event) => !(event.kind === "send" && event.value.dismissed && this.isFailedSend(event.value)));
  }
  acknowledge(receipt: MessageQueued): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === receipt.client_ref && event.value.target === receipt.target)
      ?? this.events.find((event) => event.kind === "send" && event.value.notificationId === receipt.notification_id && event.value.target === receipt.target);
    if (!send || send.kind !== "send") return false;
    send.value.notificationId = receipt.notification_id;
    send.value.stamped = receipt.stamped;
    // A late receipt means it did go: a dismissed "failed" send is back in the thread as delivered.
    delete send.value.dismissed;
    send.value.state = this.events.some((event) => event.kind === "reply" && event.value.reply_to === receipt.notification_id) ? "replied" : "acknowledged";
    delete send.value.error;
    return true;
  }
  reject(id: string, message: string): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === id);
    if (!send || send.kind !== "send" || send.value.notificationId !== undefined) return false;
    send.value.state = "error";
    send.value.error = message;
    delete send.value.held;
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
      event.value.unconfirmedAt = now;
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
  /**
   * True when the supervisor has spoken since an unconfirmed send gave up on
   * its receipt (journey F10): the send most likely arrived, so its card
   * settles instead of inviting a blind resend. A turn that arrived before
   * the give-up does not count: it is the crossing turn that made the
   * receipt overdue in the first place (cas-1185).
   */
  repliedSince(send: ConversationSend): boolean {
    const index = this.events.findIndex((event) => event.kind === "send" && event.value === send);
    if (index < 0) return false;
    const since = send.unconfirmedAt;
    return this.events.slice(index + 1).some((event) => event.kind === "reply" && (since === undefined || (event.arrivedAt ?? event.at ?? 0) > since));
  }
  /**
   * Whether `send` says "Delivered" (journey F4): a send this visit put on the
   * wire whose receipt came, until a reply linked to it (reply_to) makes it
   * "replied". An unrelated supervisor turn crossing it no longer hides the
   * tick, and a receipt that lands after such a turn still shows one.
   * Hydrated history (no sentAt) stays unmarked, so an old thread is not a
   * column of ticks.
   */
  showsDelivered(send: ConversationSend): boolean {
    return send.state === "acknowledged" && send.sentAt !== undefined;
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
      // A failed send the operator dismissed is out of the thread, so out of the preview too.
      if (event.value.dismissed && this.isFailedSend(event.value)) continue;
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
