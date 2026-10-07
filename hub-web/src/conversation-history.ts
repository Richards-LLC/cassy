import type { ConversationHistoryMessage, ConversationHistoryReply, MessageQueued, OperatorReply } from "./types";
import type { PendingSend } from "./conversation-store";

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
/**
 * cas-97d58 F18: after this long without a receipt, a live send stops saying
 * a bare "Sending…" and says what it is waiting for ("Waiting for Atlas to
 * confirm…"), long before the receipt deadline turns it Not confirmed.
 */
export const CONFIRM_CUE_MS = 4_000;

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
  /** Device that originated a durable operator turn, or Terminal. */
  deviceLabel?: string;
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
  /** Brought back from this browser's storage after a reload (cas-e7b1). */
  restored?: boolean;
}

/**
 * A restored "Not confirmed" message (cas-e7b1) is the same message as a
 * durable history row with its target and text stamped no earlier than this
 * before it went out: the machine's clock may run behind this browser's.
 */
export const RESTORED_MATCH_WINDOW_MS = 10 * 60_000;

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
 * event keeps its own time (cas-ac1f), and a reloaded turn keeps the time this
 * browser recorded even if the machine's stamp changed (cas-940f). */
/** `clockAhead` marks a live supervisor turn from a machine whose clock this
 * thread has seen running ahead: its time is the arrival time, as a reloaded
 * copy of it would show, and says so the same way (cas-1f13). */
/** `arrivedAt` is when this browser received the turn (ms epoch): a live turn
 * when it came off the socket, a durable one when its history page was
 * hydrated. It serves two readers: the receipt grace for an earlier send
 * counts from a later reply's arrival (cas-1185), and a machine stamp later
 * than the arrival shows the arrival instead, so it cannot drift with the
 * render clock (cas-1f13). */
/** `seenLive` marks a supervisor turn this browser saw arrive live, in this
 * visit or an earlier one: its arrival is a time this browser observed, so it
 * carries the mark that visit gave it instead of one derived from the
 * machine's stamp (cas-9e33). */
export type ConversationEvent = { kind: "send"; value: ConversationSend; at?: number; shownAt?: number; arrivedAt?: number; clockAhead?: boolean; seenLive?: boolean; session?: string } | { kind: "reply"; value: OperatorReply; at?: number; shownAt?: number; arrivedAt?: number; clockAhead?: boolean; seenLive?: boolean; session?: string };

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

/**
 * The short codename of a factory session (cas-55a4): `gabber-studio-calm-puma-34`
 * reads `calm-puma-34`. A name without the generated `word-word-number` tail
 * is its own codename.
 */
export function sessionCodename(session: string): string {
  return /([a-z]+-[a-z]+-\d+)$/i.exec(session)?.[1] ?? session;
}

/**
 * Another session's Commander turns (cas-55a4), kept beside the thread and
 * never in it: Commander shows them only as a labelled, collapsed section.
 * `session` is empty for turns written with no session recorded.
 */
export interface EarlierSession {
  session: string;
  /** Oldest first, by the machine's durable sequence. */
  events: ConversationEvent[];
  /** The newest turn's stamp (ms epoch), when any carries one. */
  lastAt?: number;
}

/** In-memory per-thread evidence. A submitted socket frame is never a receipt. */
export class ConversationHistory {
  readonly events: ConversationEvent[] = [];
  /** Other sessions' turns, by session (cas-55a4). Never waiting, pinned or previewed. */
  private readonly others = new Map<string, ConversationEvent[]>();
  /**
   * The supervisor session this thread is attached to now. A question stamped
   * with another session was asked by one that has since ended (cas-16eed).
   */
  currentSession?: string;
  /** Asks and blockers the operator dismissed (notification ids). */
  private readonly dismissedAsks = new Set<number>();
  /** A durable stamp from this thread's machine has been seen in this browser's future. */
  private machineAhead = false;
  /**
   * When this browser first saw each turn (cas-8d52), by `r:<id>` / `s:<id>`,
   * and the machine clock's measured lead. Kept across a reload so a rebuilt
   * thread shows each turn at the time the visit showed it.
   */
  private readonly arrivals = new Map<string, number>();
  /** Turns this page saw arrive live: only they measure the machine's lead. */
  private readonly liveArrivals = new Set<string>();
  /**
   * Supervisor turns a visit saw arrive live, and whether it marked each
   * "machine clock ahead" (cas-9e33). A reload shows the mark the visit
   * showed: a first turn from a machine not yet known to run ahead stays
   * unmarked, and one the visit marked stays marked.
   */
  private readonly liveMarks = new Map<string, boolean>();
  private skewMs?: number;

  /** Seed the times a previous visit recorded (cas-8d52). */
  seedArrivals(arrivals: { skew?: number; at: Record<string, number>; live?: Record<string, boolean> }): void {
    for (const [key, at] of Object.entries(arrivals.at)) if (!this.arrivals.has(key)) this.arrivals.set(key, at);
    for (const [key, marked] of Object.entries(arrivals.live ?? {})) if (!this.liveMarks.has(key)) this.liveMarks.set(key, marked);
    if (this.skewMs === undefined && arrivals.skew !== undefined) this.skewMs = arrivals.skew;
  }

  /** What to keep for the next visit: the newest turns' times, the measured lead and the live turns' marks. */
  arrivalsRecord(limit = 400): { skew?: number; at: Record<string, number>; live?: Record<string, boolean> } {
    const at: Record<string, number> = {};
    for (const [key, value] of [...this.arrivals].sort(([, a], [, b]) => a - b).slice(-limit)) at[key] = value;
    const live: Record<string, boolean> = {};
    for (const [key, marked] of this.liveMarks) if (key in at) live[key] = marked;
    return { ...(this.skewMs === undefined ? {} : { skew: this.skewMs }), at, ...(Object.keys(live).length ? { live } : {}) };
  }

  /**
   * The machine clock's measured lead over this browser (ms, positive when
   * ahead), once a turn seen live has come back with the machine's stamp.
   */
  machineLead(): number | undefined {
    return this.skewMs;
  }

  /**
   * The thread's newest confirmed activity, at the time the thread shows it
   * (cas-24fe): a turn's arrival here, never a machine stamp that reads in
   * this browser's future. Only confirmed turns count (cas-b00c): a supervisor
   * turn, or an operator message the machine acknowledged.
   */
  lastActivityAt(): number | undefined {
    let latest: number | undefined;
    for (const event of this.events) {
      if (event.kind === "send" && event.value.notificationId === undefined) continue;
      const at = event.shownAt ?? event.at;
      if (at === undefined || !Number.isFinite(at)) continue;
      const shown = Math.min(at, event.arrivedAt ?? at);
      if (latest === undefined || shown > latest) latest = shown;
    }
    return latest;
  }

  /**
   * The browser time a durable turn is shown at, at most `now`: the time this
   * browser first saw it, else its stamp less the machine's measured lead, else
   * `now` (cas-8d52). A stamp for a turn seen before refines the measured lead.
   */
  private arrivalFor(key: string, stamped: number | undefined, now: number): number {
    const seen = this.arrivals.get(key);
    if (seen !== undefined) {
      if (stamped !== undefined && (this.liveArrivals.has(key) || this.liveMarks.has(key))) this.skewMs = stamped - seen;
      return Math.min(seen, now);
    }
    const shown = stamped !== undefined && this.skewMs !== undefined ? Math.min(stamped - this.skewMs, now) : now;
    // Keep the time actually displayed, not a later history-page hydration
    // time: that record becomes authoritative on reload (cas-940f).
    this.arrivals.set(key, Math.min(stamped ?? shown, shown));
    return shown;
  }
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
   * A send that left this browser and has no reply yet (cas-5a8f). Unlike
   * `hasPending`, a send still held here because the machine is unreachable
   * does not count: the supervisor has not seen it, so it cannot be working
   * on it.
   */
  awaitingReply(): boolean {
    return this.events.some(event => event.kind === "send" && !event.value.held && (event.value.state === "sending" || event.value.state === "acknowledged"));
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

  /**
   * A send the hub refused as retryable (cas-0653, `upstream_unavailable`):
   * it never reached the machine's daemon, so it waits in this browser again,
   * with no receipt deadline, until it goes out once more. Returns what to
   * resend, or undefined when the send is not a live unreceipted one.
   */
  rehold(id: string): ConversationSend | undefined {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === id);
    if (!send || send.kind !== "send" || send.value.notificationId !== undefined || send.value.state !== "sending") return undefined;
    send.value.held = true;
    delete send.value.sentAt;
    return send.value;
  }

  /** The held send went out now: its receipt clock starts. */
  release(id: string, at: number = Date.now()): boolean {
    const send = this.events.find((event) => event.kind === "send" && event.value.id === id);
    if (!send || send.kind !== "send" || !send.value.held) return false;
    delete send.value.held;
    send.value.sentAt = at;
    return true;
  }

  /**
   * The operator's messages that have not settled (cas-e7b1), to keep across a
   * reload: held here, on the wire without a receipt, not confirmed, or not
   * sent. Confirmed, answered, replaced and dismissed messages are not.
   */
  pendingSends(): PendingSend[] {
    return this.events.flatMap((event): PendingSend[] => {
      if (event.kind !== "send") return [];
      const send = event.value;
      if (send.notificationId !== undefined || send.dismissed || send.replaced) return [];
      const state = send.held ? "held" : send.state === "sending" || send.state === "unconfirmed" || send.state === "error" ? send.state : undefined;
      const at = event.shownAt ?? event.at;
      if (!state || at === undefined || !Number.isFinite(at)) return [];
      return [{
        id: send.id, target: send.target, text: send.text, state, at,
        ...(send.sentAt === undefined || send.held ? {} : { sentAt: send.sentAt }),
        ...(send.replyTo === undefined ? {} : { replyTo: send.replyTo }),
        ...(send.error === undefined ? {} : { error: send.error }),
        ...(event.session === undefined ? {} : { session: event.session }),
      }];
    });
  }

  markReplyPersisted(notificationId: number): void {
    for (const event of this.events) if (event.kind === "reply" && event.value.notification_id === notificationId) { event.value.device_persisted = true; delete event.value.device_store_failed; }
  }

  /** cas-97d58 F05: this device could not keep the reply; only then does the thread say so. */
  markReplyStoreFailed(notificationId: number): void {
    for (const event of this.events) if (event.kind === "reply" && event.value.notification_id === notificationId && !event.value.device_persisted) event.value.device_store_failed = true;
  }

  /** Reconcile another tab's committed journal, without making a claim replayable. */
  synchronizePending(sends: PendingSend[], now = Date.now(), receipts: Array<MessageQueued & { sentAt: number }> = []): PendingSend[] {
    for (const receipt of receipts) {
      const event = this.events.find(event => event.kind === "send" && event.value.id === receipt.client_ref);
      if (event?.kind === "send" && event.value.target === receipt.target) {
        event.value.sentAt ??= receipt.sentAt;
        this.acknowledge(receipt);
      }
    }
    const stored = new Map(sends.map((send) => [send.id, send]));
    const reheld: PendingSend[] = [];
    for (const event of [...this.events]) {
      if (event.kind !== "send" || event.value.notificationId !== undefined || event.value.dismissed || event.value.replaced) continue;
      const current = stored.get(event.value.id);
      if (!current) {
        // A terminal journal row carries no private payload. An accepted
        // send arrives through durable history/live fan-out; a cancelled one
        // must not turn into a misleading "unsent" chip in another tab.
        this.events.splice(this.events.indexOf(event), 1);
      } else if (current.state === "held" && !event.value.held) {
        // A claimed row can be observed before the socket readiness check.
        // Authoritative no-write settlement makes that same row held again;
        // return it to the caller so the removed queue entry is restored with
        // its original journal deadline. Receipted events were excluded above.
        event.value.held = true;
        event.value.state = "sending";
        delete event.value.sentAt;
        delete event.value.unconfirmedAt;
        delete event.value.error;
        reheld.push(current);
      } else if (current.state !== "held") {
        delete event.value.held;
        // A peer's durable claim starts the same receipt clock as a local
        // write. Observing it is not evidence that delivery failed.
        event.value.state = current.state === "sending" && current.sentAt !== undefined && now < current.sentAt + RECEIPT_TIMEOUT_MS
          ? "sending" : current.state === "error" ? "error" : "unconfirmed";
        event.value.error = current.error;
        event.value.sentAt = current.sentAt;
        if (event.value.state === "unconfirmed") event.value.unconfirmedAt ??= now;
        else delete event.value.unconfirmedAt;
        event.value.restored = true;
      }
    }
    return [...reheld, ...this.restorePending(sends, now)];
  }

  /**
   * Put messages kept across a reload back in the thread (cas-e7b1), each once.
   * A held message still waits: it has never left this browser, and the
   * caller queues it to go out once. One that was on the wire without a
   * receipt keeps its original confirmation clock, then becomes "Not confirmed"
   * when that clock expires. Neither state is sent again by itself. A
   * message that was not sent stays not sent. Returns the held ones.
   */
  restorePending(sends: PendingSend[], now: number = Date.now()): PendingSend[] {
    const held: PendingSend[] = [];
    for (const stored of sends) {
      if (this.events.some((event) => event.kind === "send" && event.value.id === stored.id)) continue;
      const value: ConversationSend = { id: stored.id, target: stored.target, text: stored.text, state: "sending", restored: true, ...(stored.replyTo === undefined ? {} : { replyTo: stored.replyTo }) };
      if (stored.state === "held") {
        value.held = true;
        held.push(stored);
      } else if (stored.state === "error") {
        value.state = "error";
        value.error = stored.error ?? "This message was not sent.";
      } else if (stored.state === "sending" && stored.sentAt !== undefined && now < stored.sentAt + RECEIPT_TIMEOUT_MS) {
        value.sentAt = stored.sentAt;
      } else {
        value.state = "unconfirmed";
        value.unconfirmedAt = now;
        if (stored.sentAt !== undefined) value.sentAt = stored.sentAt;
      }
      this.insert({ kind: "send", value, at: stored.at, ...(stored.session === undefined ? {} : { session: stored.session }) });
    }
    return held;
  }

  /** The latest stamp already in the thread; live events are placed at or after it. */
  private latestAt(): number {
    return this.events.reduce((max, event) => (event.at !== undefined && Number.isFinite(event.at) && event.at > max ? event.at : max), Number.NEGATIVE_INFINITY);
  }

  /**
   * Whether a durable turn stamped with `session` belongs to another session
   * than the one this thread is attached to (cas-55a4). A turn with no session
   * field predates session stamps and stays in the thread; an empty one was
   * written with no session recorded and does not.
   */
  private foreign(session: string | undefined): boolean {
    return session !== undefined && this.currentSession !== undefined && session !== this.currentSession;
  }

  /** File another session's durable turn beside the thread, once, in its sequence. */
  private keepEarlier(event: ConversationEvent, session: string): void {
    let events = this.others.get(session);
    if (!events) this.others.set(session, (events = []));
    const seq = durableSeq(event);
    if (seq !== undefined && events.some((existing) => durableSeq(existing) === seq)) return;
    const later = seq === undefined ? -1 : events.findIndex((existing) => (durableSeq(existing) ?? Number.POSITIVE_INFINITY) > seq);
    if (later < 0) events.push(event); else events.splice(later, 0, event);
  }

  /**
   * Other sessions' turns (cas-55a4), the most recently active session first.
   * They are history from those sessions, not this one's conversation.
   */
  earlierSessions(): EarlierSession[] {
    return [...this.others].map(([session, events]) => {
      const stamps = events.flatMap((event) => event.at !== undefined && Number.isFinite(event.at) ? [event.at] : []);
      return { session, events: [...events], ...(stamps.length ? { lastAt: Math.max(...stamps) } : {}) };
    }).sort((a, b) => (b.lastAt ?? 0) - (a.lastAt ?? 0));
  }

  /** Merge one durable operator message without duplicating a live ack. */
  hydrateSend(message: ConversationHistoryMessage, now: number = Date.now()): void {
    if (this.foreign(message.session)) {
      this.keepEarlier({ kind: "send", value: { id: `history:${message.notification_id}`, target: message.target, text: message.text, state: message.state, notificationId: message.notification_id, stamped: message.stamped, deviceLabel: message.operator_label, ...(message.reply_to === undefined ? {} : { replyTo: message.reply_to }) }, at: ConversationHistory.timestamp(message.at), session: message.session }, message.session!);
      return;
    }
    const existing = this.events.find((event) => event.kind === "send" && event.value.notificationId === message.notification_id);
    if (existing?.kind === "send") {
      existing.value.target = message.target;
      existing.value.text = message.text;
      existing.value.state = message.state;
      existing.value.stamped = message.stamped;
      existing.value.deviceLabel = message.operator_label;
      // A turn already in the thread keeps what it is known to be. A
      // reconnect's history row that omits in_reply_to or the session must
      // not re-open the ask it answered or move it across the session line
      // (cas-1f13); in_reply_to never changes after the send.
      existing.value.replyTo = message.reply_to ?? existing.value.replyTo;
      existing.session ??= message.session;
      return;
    }
    const at = ConversationHistory.timestamp(message.at);
    // cas-e7b1: an unsettled message restored from the journal that the machine's
    // history now shows did arrive. It becomes that row instead of a second
    // copy of the message.
    const restored = this.events.find((event) => event.kind === "send" && event.value.restored && (event.value.state === "sending" || event.value.state === "unconfirmed") && event.value.notificationId === undefined
      && event.value.sentAt !== undefined && event.value.target === message.target && event.value.text === message.text
      && (at === undefined || at >= event.value.sentAt - RESTORED_MATCH_WINDOW_MS));
    if (restored?.kind === "send") {
      const value = restored.value;
      value.notificationId = message.notification_id;
      value.state = message.state;
      value.stamped = message.stamped;
      value.deviceLabel = message.operator_label;
      value.replyTo = message.reply_to ?? value.replyTo;
      delete value.sentAt;
      delete value.unconfirmedAt;
      delete value.restored;
      restored.session ??= message.session;
      return;
    }
    this.observeStamp(at, now);
    const shownAt = this.arrivals.get(`s:${message.notification_id}`);
    this.insertDurable({
      kind: "send",
      value: {
        id: `history:${message.notification_id}`,
        target: message.target,
        text: message.text,
        state: message.state,
        notificationId: message.notification_id,
        stamped: message.stamped,
        deviceLabel: message.operator_label,
        ...(message.reply_to === undefined ? {} : { replyTo: message.reply_to }),
      },
      at,
      ...(shownAt === undefined ? {} : { shownAt }),
      arrivedAt: this.arrivalFor(`s:${message.notification_id}`, at, now),
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
   * Whether the operator sent anything that went out (not refused) after the
   * turn with this notification id (cas-e829). That is what stops a blocker
   * waiting; it is not a reply to it unless the send carries its id.
   */
  writtenSince(notificationId: number): boolean {
    const index = this.events.findIndex((event) => event.kind === "reply" && event.value.notification_id === notificationId);
    return index >= 0 && this.events.slice(index + 1).some((later) => later.kind === "send" && later.value.state !== "error" && later.value.state !== "unconfirmed");
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
  /** A later reply quiets the warning, but its explicit Send again still retries an unknown delivery. */
  canRetrySend(send: ConversationSend): boolean {
    return send.notificationId === undefined && !send.replaced && (send.state === "error" || send.state === "unconfirmed");
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
    if (receipt.device_label) send.value.deviceLabel = receipt.device_label;
    // cas-8d52: the time this message shows, for the thread a reload rebuilds.
    const shown = send.shownAt ?? send.at;
    if (shown !== undefined && Number.isFinite(shown) && !this.arrivals.has(`s:${receipt.notification_id}`)) { this.arrivals.set(`s:${receipt.notification_id}`, shown); this.liveArrivals.add(`s:${receipt.notification_id}`); }
    // A late receipt means it did go: a dismissed "failed" send is back in the thread as delivered.
    delete send.value.dismissed;
    send.value.state = this.events.some((event) => event.kind === "reply" && event.value.reply_to === receipt.notification_id) ? "replied" : "acknowledged";
    if (send.value.held) send.value.sentAt ??= Date.now();
    delete send.value.held;
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
  /**
   * cas-a6f0: no receipt can come for sends already on the wire (the hub
   * refused this browser's pairing). They may or may not have arrived, so
   * they turn `unconfirmed` now instead of reading "Sending…" until their
   * deadline. Held sends are not on the wire and are left alone. Returns the
   * ids that changed.
   */
  unconfirmInFlight(now: number): string[] {
    const changed: string[] = [];
    for (const event of this.events) {
      if (event.kind !== "send" || event.value.state !== "sending" || event.value.held || event.value.notificationId !== undefined || event.value.sentAt === undefined) continue;
      event.value.state = "unconfirmed";
      event.value.unconfirmedAt = now;
      changed.push(event.value.id);
    }
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
  /** cas-97d58 F18: when the next live send's confirmation cue is due, for a repaint. */
  nextConfirmCue(now: number): number | undefined {
    let next: number | undefined;
    this.events.forEach((event, index) => {
      if (this.receiptDeadline(index) === undefined || event.kind !== "send" || event.value.sentAt === undefined) return;
      const cue = event.value.sentAt + CONFIRM_CUE_MS;
      if (cue > now && (next === undefined || cue < next)) next = cue;
    });
    return next === undefined ? undefined : next - now;
  }
  /** Whether a live send has waited long enough for its receipt to say so (F18). */
  awaitsConfirmation(send: ConversationSend, now: number = Date.now()): boolean {
    // Only inside the receipt window: past it the receipt check turns the
    // send Not confirmed, and the cue is not a substitute for that.
    if (send.state !== "sending" || send.held || send.notificationId !== undefined || send.sentAt === undefined) return false;
    const waited = now - send.sentAt;
    return waited >= CONFIRM_CUE_MS && waited < RECEIPT_TIMEOUT_MS;
  }
  private receiptDeadline(index: number): number | undefined {
    const event = this.events[index];
    if (event?.kind !== "send" || event.value.state !== "sending" || event.value.notificationId !== undefined || event.value.sentAt === undefined) return undefined;
    const sentAt = event.value.sentAt;
    const timeout = sentAt + RECEIPT_TIMEOUT_MS;
    // A journal projection may be on a different tab's wire. A supervisor
    // reply here cannot prove that tab's receipt is overdue. Its durable
    // settlement or the original claim timeout decides when Retry is safe.
    if (event.value.restored) return timeout;
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
      // cas-b00c: a message whose delivery was never confirmed says so in the list too.
      if (event.value.state === "unconfirmed" && !this.repliedSince(event.value)) return `Not confirmed: ${event.value.text}`;
      if (event.value.state !== "error") return `You: ${event.value.text}`;
      if (!event.value.replaced) return `Not sent: ${event.value.text}`;
    }
    return undefined;
  }
  reply(reply: OperatorReply, at: number | undefined = Date.now(), session?: string, shownAt?: number, arrivedAt: number = Date.now(), placement: "time" | "durable" = "time", clockAhead = false, seenLive = false): void {
    if (this.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) return;
    const normalized: OperatorReply = {
      ...reply,
      reply_to: reply.reply_to ?? null,
      kind: reply.kind ?? "answer",
      attachments: reply.attachments ?? [],
    };
    const event: ConversationEvent = { kind: "reply", value: normalized, at, ...(shownAt === undefined ? {} : { shownAt }), arrivedAt, ...(clockAhead ? { clockAhead } : {}), ...(seenLive ? { seenLive } : {}), session };
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
    const id = `r:${reply.notification_id}`;
    if (!this.arrivals.has(id)) { this.arrivals.set(id, at); this.liveArrivals.add(id); }
    // cas-9e33: keep the mark this visit shows, so a reload shows it too.
    if (!this.liveMarks.has(id) && !this.events.some((event) => event.kind === "reply" && event.value.notification_id === reply.notification_id)) this.liveMarks.set(id, this.machineAhead);
    const key = Math.max(at, this.latestAt());
    this.reply(reply, key, session, key === at ? undefined : at, at, "time", this.machineAhead, true);
  }

  /** Merge a durable supervisor turn in the machine's sequence, keeping its own stamp (cas-1f13). */
  hydrateReply(reply: ConversationHistoryReply, now: number = Date.now()): void {
    const { at, ...live } = reply;
    const stamped = ConversationHistory.timestamp(at);
    if (this.foreign(reply.session)) {
      this.keepEarlier({ kind: "reply", value: { ...live, reply_to: live.reply_to ?? null, kind: live.kind ?? "answer", attachments: live.attachments ?? [] }, at: stamped, session: reply.session }, reply.session!);
      return;
    }
    this.observeStamp(stamped, now);
    // cas-9e33: a turn a visit saw arrive live keeps the mark that visit gave it.
    const marked = this.liveMarks.get(`r:${reply.notification_id}`);
    const shownAt = this.arrivals.get(`r:${reply.notification_id}`);
    this.reply(live, stamped, reply.session, shownAt, this.arrivalFor(`r:${reply.notification_id}`, stamped, now), "durable", marked === true, marked !== undefined);
  }
}

/**
 * Whether the thread may say the supervisor is "working" (cas-5a8f). Real
 * pane output is independent evidence and always counts. A send awaiting its
 * reply counts only while the machine is live and paired: during an outage or
 * a revoked pairing the page cannot know, and a send held in this browser was
 * never seen at all, so "working" beside "Waiting for the connection" or
 * "Needs pairing" contradicted itself.
 */
export function supervisorWorking(
  history: Pick<ConversationHistory, "awaitingReply">,
  machine: { phase: string; authFailure?: unknown } | undefined,
  recentPaneOutput: boolean,
): boolean {
  if (recentPaneOutput) return true;
  const reachable = machine?.phase === "live" && !machine.authFailure;
  return reachable && history.awaitingReply();
}

/** The thread's paging state: how far back it has loaded, and whether more is offered. */
export interface HistoryCursor { hasEarlier: boolean; nextBefore?: number; loading: boolean; loaded: boolean }

/**
 * Fold one history page into the thread's cursor (cas-2093). Every attach
 * asks again for the newest page, and a reconnect lands that page on a thread
 * that may already reach further back, even to its start. The newest page's
 * "has earlier" is about itself, not the thread: it never brings back "Load
 * earlier" once the start was reached, nor moves the cursor forward again.
 * An older page (the one Load earlier asked for) moves the cursor back.
 */
export function applyHistoryCursor(cursor: HistoryCursor, page: { has_earlier: boolean; next_before?: number }): void {
  const first = !cursor.loaded;
  cursor.loaded = true;
  if (first) {
    cursor.loading = false;
    cursor.hasEarlier = page.has_earlier;
    cursor.nextBefore = page.next_before;
    return;
  }
  if (!page.has_earlier) {
    // This page reaches the start: the thread does too.
    cursor.loading = false;
    cursor.hasEarlier = false;
    cursor.nextBefore = undefined;
    return;
  }
  // The start was reached already: a newer page changes nothing here.
  if (!cursor.hasEarlier) return;
  const older = page.next_before !== undefined && (cursor.nextBefore === undefined || page.next_before < cursor.nextBefore);
  if (!older) return;
  cursor.loading = false;
  cursor.nextBefore = page.next_before;
}
