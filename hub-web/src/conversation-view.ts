import { fitMachineLine, hostMarkup } from "./conversation-shell";
import { joinSpoken, spokenSupervisor, supervisorDescription } from "./spoken-names";
import { machineMonogram } from "./machine-accent";
import { plainTextMarkdown, renderMarkdown } from "./markdown-renderer";
import { refusal } from "./refusal";
import { shouldFollowTail } from "./transcript";
import { bindSwipeDismiss } from "./swipe-dismiss";
import { machineName } from "./conversation-list";
import { CANT_REACH_RETRYING, NEEDS_PAIRING, UNSTEADY } from "./connection-state";
import { CONVERSATION_OPENING, OPENING_MOTION_DELAY_MS, openingLine } from "./connection-state-view";
import { sessionCodename, type ConversationEvent, type ConversationHistory, type ConversationSend, type EarlierSession } from "./conversation-history";
import type { ArtifactRef, OperatorReply, OperatorTurnKind } from "./types";
import {
  cellTone,
  coalesceText,
  dayLabel,
  stampLabel,
  messageBlocks,
  threadModel,
  type ThreadCoalesce,
  type ThreadGroup,
  type ThreadItem,
  type ThreadTurn,
} from "./thread-model";

/**
 * Pebble thread (cas-d167). The conversation shows only operator turns and
 * supervisor→operator turns; the live pane never appears here — its raw text
 * is the header's read-only Raw output drawer (cas-0546).
 */

/** What a kind-specific renderer receives. */
/** A message's place in a run of unconfirmed messages shown as one notice (cas-b00c). */
/**
 * `key`: the run's first message, and `members` every message in it
 * (cas-ca7f): Review opens this run only, and the run stays open while any
 * message the operator opened it on is still in it (dismissing one must not
 * close the rest).
 */
type UnconfirmedRun = { count: number; last: boolean; expanded: boolean; key: string; members: string[] };

export interface TurnRenderContext {
  readonly document: Document;
  readonly turn: ThreadTurn;
  readonly reply: OperatorReply;
  readonly supervisor: string;
  /** How a screen reader names this turn's supervisor: "cas-src supervisor" (cas-d8a5, journey F32). */
  readonly spokenSupervisor?: string;
  /** Renders the message body (prose + evidence tables) the way a plain bubble would. */
  readonly body: () => HTMLElement[];
  /** Set for the `attachment` kind: the artifact this call renders. */
  readonly attachment?: ArtifactRef;
  /** The thread's history: lets an ask read the send that answered it. */
  readonly history?: ConversationHistory;
  /** Sends `text` as the operator's reply to `reply` (in_reply_to = its notification_id). Absent when this view cannot send. */
  readonly respond?: (reply: OperatorReply, text: string) => void;
  /** True when rendering the copy pinned above the composer rather than the one in the flow. */
  readonly pinned?: boolean;
}

/**
 * Kinds a sibling can own. `ask` and `blocker` are supervisor turn kinds
 * (Pebble 3, the fused-tray objects); `attachment` is called once per artifact
 * on any turn that carries one (Pebble 4, the dog-eared sheet).
 */
export type RenderableKind = "ask" | "blocker" | "attachment";
export type TurnRenderer = (event: OperatorReply, context: TurnRenderContext) => HTMLElement;

const turnRenderers = new Map<RenderableKind, TurnRenderer>();

/**
 * Render hook keyed on kind — the extension point for Pebble 3 and 4.
 *
 *   registerTurnRenderer("ask", (reply, ctx) => …)        // owns the turn's silhouette
 *   registerTurnRenderer("blocker", (reply, ctx) => …)
 *   registerTurnRenderer("attachment", (reply, ctx) => …) // owns one artifact row; ctx.attachment is set
 *
 * For `ask`/`blocker` the returned element replaces the plain bubble; the view
 * still wraps it in the group's `.turn` column, so alignment, grouping and the
 * once-per-group timestamp stay the view's job, and it still stamps
 * `data-kind`/`data-reply-to`. For `attachment` the returned element replaces
 * the default link row inside the bubble. Without a registration, ask and
 * blocker fall back to a plain `.bub[data-kind]` and attachments to a link.
 * Returns a function that removes the registration.
 */
export function registerTurnRenderer(kind: RenderableKind, render: TurnRenderer): () => void {
  turnRenderers.set(kind, render);
  return () => { if (turnRenderers.get(kind) === render) turnRenderers.delete(kind); };
}

export interface ConversationViewOptions {
  /** Supervisor codename; bold in the thread header and the accessible label. */
  supervisor: string;
  /** Machine label and project name for the mono line beneath the name. */
  machine?: string;
  project?: string;
  /**
   * Machine accent class (machineAccentClass from machine-accent.ts). The shell
   * root already carries it (conversation-shell.ts) so the thread inherits the
   * accent; pass it here only when the view is mounted outside that shell.
   */
  accentClass?: string;
  /** True while the supervisor is executing: paints the working line. */
  working?: () => boolean;
  /**
   * Render the thread's own header. Inside the conversation shell the shell
   * supplies the single Pebble header (conversationHeaderMarkup), so main.ts
   * and the fixture pass false; a standalone mount keeps the default.
   */
  header?: boolean;
  /**
   * The last thing said in this conversation, echoed faintly under the
   * nothing-waiting line when the thread has no turns to show (Pebble 4).
   */
  echo?: () => string | undefined;
  /**
   * Refused sends offer to put their text back into the composer. The send is
   * passed too, so the caller can retire it once the edited version goes out.
   */
  editMessage?: (text: string, send: ConversationSend) => void;
  /** Refused sends offer to go out again unchanged (same text, same in_reply_to). */
  retryMessage?: (send: ConversationSend) => void;
  cancelMessage?: (send: ConversationSend) => void;
  /**
   * A send refused because this device does not control the session offers
   * Take control on the message itself, beside Retry (cas-3433): the refusal
   * tells the operator to take control, so the control sits where they read it.
   */
  takeControl?: (send: ConversationSend) => void;
  /**
   * Whether this device controls the session now. Once it does, a control
   * refusal stops offering Take control and says Retry will go through
   * (cas-8e0a); if control is lost again, both come back.
   */
  controlHeld?: () => boolean;
  /**
   * Whether the conversation's session is live now (cas-d15c). A send that
   * expired waiting for it then says Retry will go through, instead of
   * "once the session is live again" under a header that reads Live.
   */
  sessionLive?: () => boolean;
  /**
   * The device holding control when this one cannot take it over. A control
   * refusal then names it and says to take control once it is released, as
   * the composer does (cas-1730, cas-008f N01). Take control stays on the
   * message so the reader keeps their place (cas-008f).
   */
  controlHolder?: () => string | undefined;
  /**
   * Quick replies and composer replies to an ask go through this; the caller
   * sends with in_reply_to = the ask's notification_id and records the send
   * in the history with the same replyTo, which is what marks the ask answered.
   */
  respond?: (ask: OperatorReply, text: string) => void;
  /**
   * What the session is doing while it has said nothing to Commander
   * (cas-55a4): when it last did anything and between whom. The empty
   * thread shows it, so a live session never reads as idle or as a copy.
   */
  activity?: () => { at?: number; label?: string; terminal?: boolean } | undefined;
  /**
   * The conversation header's connection label ("Live", "Degraded",
   * "Reconnecting", "Needs pairing", …). The empty thread reads it, so it
   * never promises new messages over a connection that cannot carry them
   * (cas-010f). Unset reads as live.
   */
  connection?: () => string | undefined;
  /** Requests the next older durable page when history has more turns. */
  loadEarlier?: () => void;
  /** Whether the daemon reported an older page still available. */
  hasEarlier?: () => boolean;
  /** Keeps the paging control honest while a request is in flight. */
  loadingEarlier?: () => boolean;
  /**
   * The first history page is on its way: an empty thread shows a loading
   * line rather than claiming nothing is waiting (cas-04ee).
   */
  loadingHistory?: () => boolean;
  /**
   * When this conversation began to open (cas-813a). The loading line keeps
   * the attach's motion clock, so it doesn't start its motion over.
   */
  openingSince?: () => number | undefined;
  /** The loaded page reaches the beginning of the project history. */
  historyEnd?: () => boolean;
  /**
   * The operator dismissed or restored a failed send, or dismissed a question
   * (cas-16eed). The caller refreshes what reads the history beside the thread
   * (the list preview, the composer's pointer at the refused message) and
   * keeps the dismissed questions across a reload.
   */
  dismissalsChanged?: () => void;
}

const TICK = '<svg class="tick" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.6 8.6l3.3 3.3L13.4 4.4"/></svg>';
/** Warning triangle for a refused send; decorative — the "Not sent" text carries the meaning. */
const CLOSE = '<svg class="close" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8"/></svg>';
const CHEVRON_UP = '<svg class="chevron" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 10l4-4 4 4"/></svg>';

/**
 * The line the collapsed question bar shows (cas-16eed): the question itself
 * when the message has one ("open the PR to main and cut a release?" out of
 * "- **Ask:** open the PR…"), else its first line. Markdown marks and a
 * leading "Ask:" / "Question:" label are dropped; the bar ellipsises.
 */
export function askLine(message: string): string {
  const lines = message.split(/\r?\n/).map((line) => plainTextMarkdown(line)
    .replace(/^\s*(?:[-*•+]|\d+[.)])\s+/, "")
    .replace(/^\s*(?:ask|question|decision needed|decision)\s*:\s*/i, "")
    .trim()).filter(Boolean);
  const question = [...lines].reverse().find((line) => line.endsWith("?"));
  return question ?? lines[0] ?? "";
}

/**
 * The block in a rendered ask that carries its question (the line `askLine`
 * names), innermost and last when several match.
 */
export function askLineElement(body: HTMLElement, message: string): HTMLElement | undefined {
  const question = askLine(message);
  if (!question) return undefined;
  const blocks = [...body.querySelectorAll<HTMLElement>("p, li, h1, h2, h3, h4, h5, h6, blockquote")];
  return blocks.reverse().find((block) => askLine(block.textContent ?? "") === question);
}

/**
 * cas-8674 (journey F6): an opened pinned card's body scrolls inside a
 * quarter of the screen, and a question usually ends its message, under the
 * preamble. Scroll the body (only the body, never the page) so the question
 * sits in view beside its choices; one taller than the body shows its start.
 */
export function revealAskLine(body: HTMLElement, message: string): void {
  if (body.scrollHeight <= body.clientHeight) return;
  const line = askLineElement(body, message);
  if (!line) return;
  const style = body.ownerDocument.defaultView?.getComputedStyle(body);
  const padTop = parseFloat(style?.paddingTop ?? "") || 0;
  const padBottom = parseFloat(style?.paddingBottom ?? "") || 0;
  const box = body.getBoundingClientRect();
  const rect = line.getBoundingClientRect();
  const offset = rect.top - box.top - body.clientTop + body.scrollTop;
  body.scrollTop = Math.max(0, Math.min(offset - padTop, offset + rect.height + padBottom - body.clientHeight));
}

const WARN = '<svg class="warn" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 1.9 14.6 13.6H1.4Z"/><path d="M8 6.2v3.4"/><path d="M8 11.7v.1"/></svg>';

/**
 * Give focus back inside a rebuilt bubble (cas-8e0a F01). The same control
 * keeps it when it survived the rebuild. Otherwise Retry takes it (after
 * Take control, Retry is the next step), then any action, then the message
 * itself. Never the page body.
 */
function landFocusIn(bubble: HTMLElement, className: string): void {
  const same = className ? [...bubble.querySelectorAll<HTMLElement>("button")].find((button) => button.className === className) : undefined;
  const target = same ?? bubble.querySelector<HTMLElement>(".conversation-retry") ?? bubble.querySelector<HTMLElement>("button");
  if (target) { target.focus({ preventScroll: true }); return; }
  bubble.tabIndex = -1;
  bubble.focus({ preventScroll: true });
}

/** The header's connection label, as the empty thread reads it. */
function connectionKind(label: string | undefined): "live" | "degraded" | "pairing" | "reconnecting" | "unreachable" {
  return label === undefined || label === "Live" ? "live"
    // cas-97d58 F10: the half-open header's "Unsteady" is the banner's
    // "unsteady — checking…", not "can't be reached".
    : label === "Degraded" || label === UNSTEADY ? "degraded"
      : label === NEEDS_PAIRING ? "pairing"
        : label === "Reconnecting" || label === "Connecting" || label === "Idle" || label === CANT_REACH_RETRYING ? "reconnecting"
          : "unreachable";
}

/**
 * What the empty thread says (cas-010f), from the header's connection label
 * and whether this session's first history page has resolved:
 * - `loading`: the page is on its way over a connection that can bring it;
 * - `waiting`: it cannot arrive until the connection is back, so the card
 *   says why instead of claiming there is nothing;
 * - `empty`: the page resolved with no turns of this session's own.
 * Plain words only: no product codename, and the generated session codename
 * stays in the card's meta line.
 */
export function emptyThreadCopy(input: { project?: string; machine?: string; connection?: string; resolved: boolean }): { state: "loading" | "waiting" | "empty"; said: string } {
  const subject = input.project ? `the ${input.project} supervisor` : "this supervisor";
  const where = input.machine || "this machine";
  const subjectMachine = input.machine || "This machine";
  const label = input.connection;
  const kind = connectionKind(label);
  const none = `No messages from ${subject} in this session yet`;
  if (!input.resolved) {
    if (kind === "live" || kind === "degraded" || label === "Connecting" || label === "Idle") return { state: "loading", said: "" };
    if (kind === "pairing") return { state: "waiting", said: `${subjectMachine} needs pairing again before messages from ${subject} can load.` };
    if (kind === "reconnecting") return { state: "waiting", said: `Reconnecting to ${where} — messages from ${subject} will load once it's back.` };
    return { state: "waiting", said: `${subjectMachine} can't be reached — messages from ${subject} will load once it's back.` };
  }
  if (kind === "live") return { state: "empty", said: `${none} — nothing is waiting on you.` };
  if (kind === "degraded") return { state: "empty", said: `${none}. The connection is unsteady, so a new one may arrive late.` };
  if (kind === "pairing") return { state: "empty", said: `${none}. ${subjectMachine} needs pairing again before new ones can arrive.` };
  if (kind === "reconnecting") return { state: "empty", said: `${none}. Reconnecting to ${where} — anything new will show here once it's back.` };
  return { state: "empty", said: `${none}. ${subjectMachine} can't be reached — anything new will show here once it's back.` };
}

/**
 * The empty thread's activity line (cas-010f): plain words, no queue jargon.
 * The supervisor's terminal output when that is newest; otherwise the
 * session's own last activity.
 */
export function emptyCardActivityText(activity: { at?: number; terminal?: boolean }, now: number): string {
  if (activity.at === undefined) return "";
  return `${activity.terminal ? "Terminal output" : "Last active"} ${relativeAgo(activity.at, now)}`;
}

/** "Last activity 3m ago · supervisor → worker-1": a conversation row's title (cas-55a4). */
export function emptyActivityText(activity: { at?: number; label?: string }, now: number): string {
  const when = activity.at === undefined ? undefined : relativeAgo(activity.at, now);
  return ["Last activity", when, activity.label ? `· ${activity.label}` : undefined].filter(Boolean).join(" ");
}

function relativeAgo(at: number, now: number): string {
  const elapsed = Math.max(0, now - at);
  if (elapsed < 60_000) return "just now";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)}m ago`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)}h ago`;
  return `${Math.floor(elapsed / 86_400_000)}d ago`;
}

/** Name the question an answer belongs to; unavailable history stays explicit. */
export function earlierReplyQuote(document: Document, session: string, question?: string): HTMLElement {
  const quote = document.createElement("div");
  quote.className = "reply-quote";
  const firstLine = question ? plainTextMarkdown(question.split(/\r?\n/, 1)[0] ?? "").trim() : "";
  quote.textContent = firstLine
    ? `Reply to “${firstLine}” · ${sessionCodename(session)}`
    : `Reply to your message in earlier session ${sessionCodename(session)}`;
  quote.title = session;
  return quote;
}

/** "Earlier session calm-puma-34, Yesterday" (cas-55a4). */
export function earlierSessionLabel(entry: Pick<EarlierSession, "session" | "lastAt">, now: number): string {
  const when = entry.lastAt === undefined ? undefined : dayLabel(entry.lastAt, now);
  if (!entry.session) return `Earlier messages with no session recorded${when ? `, ${when}` : ""}`;
  return `Earlier session ${sessionCodename(entry.session)}${when ? `, ${when}` : ""}`;
}

/** One collapsed earlier session: its label, then its turns with their day and time. */
function earlierSessionNode(document: Document, entry: EarlierSession, now: number, open: boolean): HTMLElement {
  const details = document.createElement("details"); details.className = "earlier-session";
  details.dataset.session = entry.session;
  details.open = open;
  const summary = document.createElement("summary");
  const label = document.createElement("span"); label.className = "earlier-label"; label.textContent = earlierSessionLabel(entry, now);
  const count = document.createElement("span"); count.className = "earlier-count";
  count.textContent = `${entry.events.length} message${entry.events.length === 1 ? "" : "s"}`;
  summary.append(label, count);
  const list = document.createElement("ol"); list.className = "earlier-turns";
  for (const event of entry.events) {
    const item = document.createElement("li"); item.className = `earlier-turn ${event.kind === "send" ? "you" : "supervisor"}`;
    // cas-8d52: an earlier session's supervisor by its own codename.
    const who = document.createElement("b"); who.textContent = event.kind === "send" ? (event.value.deviceLabel ? `You · ${event.value.deviceLabel}` : "You") : entry.session ? sessionCodename(entry.session) : "Supervisor";
    const time = document.createElement("time");
    if (event.at !== undefined && Number.isFinite(event.at)) {
      time.dateTime = new Date(event.at).toISOString();
      time.textContent = stampLabel(event.at, now) ?? "";
    }
    const text = document.createElement("p");
    text.textContent = plainTextMarkdown(event.kind === "send" ? event.value.text : event.value.message);
    item.append(who, " ", time, text);
    list.append(item);
  }
  details.append(summary, list);
  return details;
}

/**
 * The dismissed-messages chip's words (cas-6a96, journey F36). It counts the
 * messages the operator dismissed, so it says "dismissed": a thread notice
 * beside it counts the not-confirmed messages still in the thread, and the
 * two used to read as contradicting counts of the same thing ("2 messages
 * not confirmed" over "Show 3 messages not confirmed"). Messages known not to
 * have gone keep their plain "unsent" name.
 */
export function unsentChipCopy(count: number, unconfirmed: number): { text: string; label: string } {
  if (unconfirmed === 0) {
    const noun = count === 1 ? "1 unsent message" : `${count} unsent messages`;
    return { text: noun, label: `Show ${noun}` };
  }
  const messages = count === 1 ? "message" : "messages";
  const status = unconfirmed === count ? "not confirmed" : "not sent or not confirmed";
  return { text: `${count} dismissed`, label: `Show ${count} dismissed ${messages}, ${status}` };
}

export class ConversationView {
  readonly element: HTMLElement;
  /**
   * A compact bookmark above the composer points to the complete question
   * in the flow. It hides when no ask is waiting and clears on answer.
   */
  readonly pinned: HTMLElement;
  private readonly head: HTMLElement;
  private readonly loadEarlier: HTMLButtonElement;
  private readonly msgs: HTMLElement;
  private readonly empty: HTMLElement;
  /** Other sessions' turns, collapsed and labelled (cas-55a4); never part of the log. */
  private readonly earlier: HTMLElement;
  /**
   * "Jump to latest", shown while the reader is scrolled away from the tail.
   * Mount it above the composer (as main.ts does), outside the scrolling
   * thread, so it takes its own row instead of floating over a turn (cas-97ea).
   */
  readonly jump: HTMLButtonElement;
  /**
   * "1 unsent message": shown while failed sends are dismissed (cas-16eed).
   * Mount it above the composer beside Jump to latest; pressing it brings
   * them back with their Edit and Retry.
   */
  readonly unsent: HTMLButtonElement;
  private readonly options: ConversationViewOptions;
  /** Composer focus/keyboard state, used to keep the thread tail visible. */
  private composing = false;
  private nodes = new Map<string, HTMLElement>();
  /** Coalesced status lines the operator opened with "Show full update"; survives repaints. */
  private expanded = new Set<string>();
  /**
   * cas-b00c: messages in the unconfirmed runs the operator opened with
   * Review (cas-ca7f): a run is open while any of its messages is here, so a
   * later, separate run starts as one notice instead of opening already
   * expanded, and dismissing one message keeps the rest of its run open.
   */
  private readonly reviewedRuns = new Set<string>();
  private following = true;
  /**
   * Where the reader was, by turn, while not following the tail (cas-2093).
   * A reconnect rebuilds the pane card around this view, and the browser
   * resets the moved thread's scroll to the top; this puts the reader back.
   */
  private place?: { key: string; offset: number };
  private placePending = false;
  /** The scroll position this view last saw or set; another value means the browser reset it. */
  private scrolledTo = 0;
  /**
   * cas-71af (1584 QA F01): Load earlier held focus when pressed. It is
   * disabled while the page loads and hidden once the last page lands, and
   * either used to drop focus to the page body.
   */
  private loadEarlierFocus = false;
  /**
   * cas-c2cb: a control in the thread the reader moved focus to. Their focus
   * scrolled it into view, so it outranks a pending put-back of the reading
   * position (placePending), which would scroll it back out of the thread.
   * The same control given focus back after a rebuild (recovery) is not a move,
   * and neither is focus this view places itself.
   */
  private focusMoved?: HTMLElement;
  /** The control in the thread that last held focus, kept across a re-mount's drop to <body>. */
  private focusedControl?: HTMLElement;
  /** Set while this view places focus itself (Load earlier's own recovery). */
  private placingFocus = false;
  private pinPending = false;
  /** The thread's height at the last scroll or resize it saw (cas-16eed). */
  private lastHeight?: number;
  /** The thread's content height at the last scroll it saw (cas-acb4b). */
  private lastContentHeight?: number;
  private disposed = false;
  private resize?: ResizeObserver;

  constructor(document: Document, private history: ConversationHistory, options: ConversationViewOptions | string, editMessage?: (text: string) => void) {
    this.options = typeof options === "string" ? { supervisor: options, editMessage } : options;
    const { supervisor } = this.options;
    this.element = document.createElement("div");
    this.element.className = "conversation-reading thread";
    if (this.options.accentClass) this.element.classList.add(this.options.accentClass);
    this.element.tabIndex = 0;
    this.element.setAttribute("aria-label", `Conversation with ${supervisor}`);
    this.head = document.createElement("header"); this.head.className = "thead";
    const identity = document.createElement("div"); identity.className = "id";
    const name = document.createElement("b"); name.className = "codename"; name.textContent = supervisor;
    const where = document.createElement("span");
    where.textContent = [this.options.project, this.options.machine].filter(Boolean).join(" · ");
    identity.append(name, where);
    this.head.append(identity);
    this.loadEarlier = document.createElement("button");
    this.loadEarlier.type = "button";
    this.loadEarlier.className = "conversation-load-earlier";
    this.loadEarlier.textContent = "Load earlier";
    this.loadEarlier.hidden = true;
    // Asking for older turns is reading, not following the tail: the page
    // lands above and the turn on screen stays put (journey F7).
    this.loadEarlier.onclick = () => {
      this.following = false;
      // Paging holds the reader's turn from here (journey F7), not the button.
      this.focusMoved = undefined;
      this.loadEarlierFocus = this.element.ownerDocument.activeElement === this.loadEarlier;
      this.options.loadEarlier?.();
    };
    this.msgs = document.createElement("div"); this.msgs.className = "msgs";
    this.msgs.setAttribute("role", "log");
    this.empty = document.createElement("div"); this.empty.className = "empty"; this.empty.hidden = true;
    this.jump = document.createElement("button"); this.jump.type = "button";
    this.jump.className = "conversation-jump"; this.jump.textContent = "Jump to latest"; this.jump.hidden = true;
    this.jump.onclick = () => { this.following = true; this.focusMoved = undefined; this.update(); this.pin(); };
    this.unsent = document.createElement("button"); this.unsent.type = "button";
    this.unsent.className = "conversation-unsent"; this.unsent.hidden = true;
    this.unsent.onclick = () => this.restoreUnsent();
    this.earlier = document.createElement("section"); this.earlier.className = "earlier-sessions"; this.earlier.hidden = true;
    this.earlier.setAttribute("aria-label", "Earlier sessions");
    this.element.append(...(this.options.header === false ? [] : [this.head]), this.earlier, this.loadEarlier, this.msgs, this.empty, this.jump);
    this.pinned = document.createElement("div"); this.pinned.className = "pinned-ask"; this.pinned.hidden = true;
    bindSwipeDismiss(this.pinned, { onDismiss: () => { const ask = this.history.pinnedAsk(); if (ask) this.dismissAsk(ask.notification_id, false); } });
    if (this.options.accentClass) this.pinned.classList.add(this.options.accentClass);
    this.pinned.setAttribute("role", "region"); this.pinned.setAttribute("aria-label", `Waiting on you: question from ${supervisor}`);
    this.element.addEventListener("scroll", () => {
      if (this.pinPending) return;
      // cas-16eed: the phone keyboard shrinks the thread, and the browser's
      // own scroll adjustment for that lands here before the resize observer
      // does. A scroll that comes with a height change is layout, not the
      // reader scrolling away: a thread following its tail keeps following.
      const height = this.element.clientHeight;
      if (this.lastHeight !== undefined && height !== this.lastHeight) {
        this.lastHeight = height;
        if (this.following) { this.pin(); return; }
      }
      this.lastHeight = height;
      // A scroll that comes with a change of content height while following
      // is layout (content grew, the browser re-anchored), not the reader.
      const contentHeight = this.element.scrollHeight;
      const grew = this.lastContentHeight !== undefined && contentHeight !== this.lastContentHeight;
      this.lastContentHeight = contentHeight;
      if (grew && this.following) { this.pin(); return; }
      this.following = shouldFollowTail(this.element);
      this.jump.hidden = this.following;
      this.scrolledTo = this.element.scrollTop;
      // The reader scrolled their focused control away: their scroll is the place now.
      if (this.focusMoved && !this.shows(this.focusMoved)) this.focusMoved = undefined;
      this.notePlace();
    }, { passive: true });
    this.element.addEventListener("focusin", (event) => {
      const target = event.target;
      if (!(target instanceof HTMLElement) || target === this.element) return;
      if (target !== this.focusedControl && !this.placingFocus) {
        this.focusMoved = target;
        // The reader moved to a control the thread does not show, or one a
        // thread following its tail would pin out of view (Load earlier, above,
        // just after a reconnect reset the scroll to the top). The browser
        // shows it; pinning to the tail, or putting a reconnect's reading
        // position back, would scroll it straight out again. The reader's
        // focus is the place now.
        if (!this.shows(target) || (this.following && this.hiddenAtTail(target))) {
          this.following = false;
          this.jump.hidden = false;
          this.placePending = false;
          this.place = undefined;
        }
      }
      this.focusedControl = target;
    });
    this.element.addEventListener("focusout", (event) => {
      const next = event.relatedTarget;
      // No next target: a re-mount dropped it, and recovery gives the same control focus back.
      if (!(next instanceof Node) || this.element.contains(next)) return;
      this.focusedControl = undefined;
      this.focusMoved = undefined;
    });
    if (typeof ResizeObserver !== "undefined") {
      this.resize = new ResizeObserver(() => {
        for (const node of this.msgs.querySelectorAll<HTMLElement>(".coalesce-turn")) syncClampPill(node);
        this.lastHeight = this.element.clientHeight;
        this.fitEmptyMeta();
        this.noticeReset();
        this.restorePlace();
        if (this.following) this.pin();
      });
      this.resize.observe(this.element);
      // cas-acb4b: the thread's content grows after it was pinned (file cards
      // and late text layout finish after the first paint) while the scroll
      // box keeps its size. Watching only the box left a thread that was
      // following its tail a few exchanges above it, and the browser's next
      // scroll-anchoring nudge then read as the reader scrolling away.
      this.resize.observe(this.msgs);
    }
  }

  /** Re-derive the thread from the history; nodes are keyed so grouping survives. */
  update(): void {
    if (this.disposed) return;
    this.noticeReset();
    this.restorePlace();
    // Turns added above the reader (Load earlier) must not move what they are
    // reading (journey F7): remember the first turn on screen and where it sat.
    const anchor = this.following ? undefined : this.readingAnchor();
    const offeredEarlier = !this.loadEarlier.hidden;
    const hasEarlier = this.options.hasEarlier?.() === true;
    const loadingEarlier = this.options.loadingEarlier?.() === true;
    this.loadEarlier.hidden = !hasEarlier;
    this.renderLoadEarlier(loadingEarlier);
    const working = this.options.working?.() === true;
    // A dismissed failed send leaves the thread (cas-16eed); the unsent chip brings it back.
    const model = threadModel(this.history.visibleEvents(), { working, historyEnd: this.options.historyEnd?.() === true, session: this.history.currentSession });
    const document = this.element.ownerDocument;
    const next = new Map<string, HTMLElement>();
    const children: HTMLElement[] = [];
    for (const item of model) {
      let node = this.nodes.get(item.key);
      if (!node) node = document.createElement(item.type === "day" ? "p" : "div");
      this.renderItem(node, item);
      next.set(item.key, node);
      children.push(node);
    }
    this.nodes = next;
    for (const key of this.expanded) if (!next.has(key)) this.expanded.delete(key);
    // Only re-append when the sequence changed: an unchanged list keeps its
    // scroll position and selection.
    const same = this.msgs.children.length === children.length && children.every((node, index) => this.msgs.children[index] === node);
    if (!same) this.msgs.replaceChildren(...children);
    this.renderPinned(document);
    this.renderUnsent();
    this.renderEmpty(model.length === 0, this.options.loadingHistory?.() === true);
    this.renderEarlier(model.length === 0);
    const held = anchor && this.anchorNode(anchor);
    if (anchor && held) {
      // The button sits above every turn, so the browser's own scroll
      // anchoring has nothing to hold on to; hold the turn ourselves.
      const drift = held.getBoundingClientRect().top - anchor.top;
      if (Math.abs(drift) >= 1) this.element.scrollTop += drift;
      // cas-2093 (F12): the last page landed. The end of history is stated on
      // screen, not left above the fold.
      if (offeredEarlier && !hasEarlier) this.revealHistoryEnd();
    }
    this.restorePlace();
    this.scrolledTo = this.element.scrollTop;
    this.lastContentHeight = this.element.scrollHeight;
    this.notePlace();
    if (this.following && document.getSelection()?.isCollapsed !== false) this.pin();
    if (this.loadEarlierFocus && !loadingEarlier) this.restoreLoadEarlierFocus(document);
  }

  /**
   * Once the page it asked for lands, Load earlier gets its focus back; after
   * the last page, where it hides, focus goes to the "No earlier history"
   * line (else the oldest turn) at the top, where the button was. A reader
   * who moved focus elsewhere meanwhile keeps it there.
   */
  private restoreLoadEarlierFocus(document: Document): void {
    const active = document.activeElement;
    const lost = !active || active === document.body || active === this.loadEarlier;
    if (!this.loadEarlier.hidden) {
      if (!lost) this.loadEarlierFocus = false;
      else if (active !== this.loadEarlier) this.placeFocus(this.loadEarlier);
      return;
    }
    this.loadEarlierFocus = false;
    if (!lost) return;
    const target = this.msgs.querySelector<HTMLElement>(":scope > .history-end") ?? this.msgs.querySelector<HTMLElement>("[data-key]");
    if (!target) { this.element.focus({ preventScroll: true }); return; }
    target.tabIndex = -1;
    this.placeFocus(target);
  }

  /** Focus this view places itself, without scrolling: not the reader moving it (cas-c2cb). */
  private placeFocus(target: HTMLElement): void {
    this.placingFocus = true;
    try { target.focus({ preventScroll: true }); } finally { this.placingFocus = false; }
  }

  /** Whether the element sits wholly above the thread's last screenful, where following the tail cannot show it. */
  private hiddenAtTail(element: HTMLElement): boolean {
    const view = this.element.getBoundingClientRect();
    const bottom = element.getBoundingClientRect().bottom - view.top + this.element.scrollTop;
    return bottom <= this.element.scrollHeight - this.element.clientHeight;
  }

  /** Whether any of the element shows inside the thread's scroll box. */
  private shows(element: HTMLElement): boolean {
    const view = this.element.getBoundingClientRect();
    const box = element.getBoundingClientRect();
    return box.height > 0 && box.bottom > view.top && box.top < view.bottom;
  }

  /**
   * The control the reader moved focus to, while it still has focus (cas-c2cb).
   * A re-mount drops focus to <body> for a moment before recovery gives it
   * back, so that does not count as the reader moving on.
   */
  private focusHeld(): HTMLElement | undefined {
    const target = this.focusMoved;
    const active = target?.ownerDocument.activeElement;
    if (target && target.isConnected && this.element.contains(target)
      && (active === target || !active || active === target.ownerDocument.body)) return target;
    this.focusMoved = undefined;
    return undefined;
  }

  /**
   * The first turn still on screen (by its key), and its top edge, before a
   * repaint. Keyed bubbles, not top-level items: an older page's turns from
   * the same sender join the reader's group, which is rebuilt, and an older
   * page from the same day slots in under the day line, which stays put.
   */
  private readingAnchor(): { key?: string; node: HTMLElement; top: number } | undefined {
    if (this.msgs.hidden) return undefined;
    const top = this.element.getBoundingClientRect().top;
    const visible = (node: HTMLElement) => { const box = node.getBoundingClientRect(); return box.height > 0 && box.bottom > top ? box.top : undefined; };
    for (const node of this.msgs.querySelectorAll<HTMLElement>("[data-key]")) {
      const at = visible(node);
      if (at !== undefined) return { key: node.dataset.key, node, top: at };
    }
    for (const child of this.msgs.children) {
      const at = visible(child as HTMLElement);
      if (at !== undefined) return { node: child as HTMLElement, top: at };
    }
    return undefined;
  }

  /**
   * The last page the reader asked for is above them; they asked for it to
   * read it. Scroll up just enough that its first line, the end of history,
   * is at the top: the page reads down to where they were.
   */
  private revealHistoryEnd(): void {
    const end = this.msgs.querySelector<HTMLElement>(":scope > .history-end");
    if (!end) return;
    const view = this.element.getBoundingClientRect();
    const above = view.top - end.getBoundingClientRect().top;
    if (above <= 0) return;
    this.element.scrollTop -= above;
  }

  /** Remember the reader's turn and its offset from the top of the thread (cas-2093). */
  private notePlace(): void {
    if (this.following) { this.place = undefined; return; }
    if (!this.element.isConnected || this.element.clientHeight === 0 || this.placePending) return;
    const anchor = this.readingAnchor();
    if (anchor?.key === undefined) return;
    this.place = { key: anchor.key, offset: anchor.top - this.element.getBoundingClientRect().top };
  }

  /**
   * A thread taken out of the page and put back (a reconnect rebuilds the
   * pane card around it) comes back scrolled to its top, with no scroll event
   * to say so. A position this view did not see or set is that reset: put the
   * reader back (cas-2093).
   */
  private noticeReset(): void {
    if (!this.element.isConnected || this.element.clientHeight === 0) return;
    if (Math.abs(this.element.scrollTop - this.scrolledTo) > 1) this.placePending = true;
  }

  private restorePlace(): void {
    if (!this.placePending || !this.element.isConnected || this.element.clientHeight === 0) return;
    if (this.following) { this.placePending = false; this.pin(); return; }
    const place = this.place;
    if (!place) { this.placePending = false; return; }
    const node = [...this.msgs.querySelectorAll<HTMLElement>("[data-key]")].find((item) => item.dataset.key === place.key);
    // Not drawn yet: try again on the next update.
    if (!node) return;
    this.placePending = false;
    // cas-c2cb: the reader tabbed to a control here after the reset, and their
    // focus scrolled it into view. If putting the old position back would
    // scroll that control out of the thread, their focus is the place now.
    const focused = this.focusHeld();
    const before = this.element.scrollTop;
    const shownBefore = focused !== undefined && this.shows(focused);
    const offset = node.getBoundingClientRect().top - this.element.getBoundingClientRect().top;
    this.element.scrollTop += offset - place.offset;
    if (focused && shownBefore && !this.shows(focused)) {
      this.element.scrollTop = before;
      this.scrolledTo = before;
      this.notePlace();
    }
  }

  private anchorNode(anchor: { key?: string; node: HTMLElement }): HTMLElement | undefined {
    if (anchor.key !== undefined) {
      for (const node of this.msgs.querySelectorAll<HTMLElement>("[data-key]")) if (node.dataset.key === anchor.key) return node;
    }
    return anchor.node.isConnected ? anchor.node : undefined;
  }

  /**
   * While an older page loads the button stays readable (it is the only
   * loading signal): ink-mid at full opacity, the thread's three-dot working
   * mark beside the label, and aria-busy on the thread for assistive tech.
   */
  private renderLoadEarlier(loading: boolean): void {
    if (loading) this.element.setAttribute("aria-busy", "true");
    else this.element.removeAttribute("aria-busy");
    if (this.loadEarlier.disabled === loading && this.loadEarlier.dataset.loading === String(loading)) return;
    this.loadEarlier.disabled = loading;
    this.loadEarlier.dataset.loading = String(loading);
    const document = this.element.ownerDocument;
    if (!loading) { this.loadEarlier.textContent = "Load earlier"; return; }
    const dots = document.createElement("span"); dots.className = "dots"; dots.setAttribute("aria-hidden", "true");
    dots.append(document.createElement("i"), document.createElement("i"), document.createElement("i"));
    this.loadEarlier.replaceChildren(dots, document.createTextNode("Loading earlier…"));
  }

  /**
   * The most recent unanswered ask, bookmarked above the composer; answering,
   * dismissing or retiring it unpins it (cas-16eed). Collapsed, it is a
   * one-line bar naming the question; expanded, the whole card with its
   * choices. Either way it offers Dismiss, and on a touch screen it swipes off.
   */
  private renderPinned(document: Document): void {
    const ask = this.history.pinnedAsk();
    if (!ask || !turnRenderers.has("ask")) {
      this.pinned.hidden = true; this.pinned.replaceChildren(); delete this.pinned.dataset.signature;
      return;
    }
    const signature = JSON.stringify([ask.notification_id, ask.message]);
    if (!this.pinned.hidden && this.pinned.dataset.signature === signature) return;
    const focused = document.activeElement instanceof HTMLElement && this.pinned.contains(document.activeElement)
      ? document.activeElement.className : undefined;
    this.pinned.dataset.signature = signature;
    this.pinned.dataset.collapsed = "true";
    const bar = document.createElement("div"); bar.className = "pinned-bar";
    const jump = document.createElement("button"); jump.type = "button"; jump.className = "pinned-expand";
    const label = document.createElement("span"); label.className = "pinned-bar-label"; label.textContent = "Waiting on you:";
    const text = document.createElement("span"); text.className = "pinned-bar-text"; text.textContent = askLine(ask.message);
    const glyph = document.createElement("template"); glyph.innerHTML = CHEVRON_UP;
    jump.append(label, " ", text, glyph.content.firstElementChild!);
    jump.title = "Go to the question";
    jump.onclick = () => {
      const turn = this.element.querySelector<HTMLElement>(`[data-key="reply:${ask.notification_id}"]`);
      if (turn) {
        turn.tabIndex = -1;
        turn.scrollIntoView?.({ block: "center" });
        turn.focus({ preventScroll: true });
      }
      // Only the latest ask owns the shared composer's reply target.
      if (!(ask.options ?? []).some(option => option.trim())) document.getElementById("message-text")?.focus({ preventScroll: true });
    };
    const dismiss = document.createElement("button"); dismiss.type = "button"; dismiss.className = "pinned-dismiss";
    dismiss.setAttribute("aria-label", "Dismiss question"); dismiss.title = "Dismiss question"; dismiss.innerHTML = CLOSE;
    dismiss.onclick = () => this.dismissAsk(ask.notification_id, true);
    bar.append(jump, dismiss); this.pinned.replaceChildren(bar); this.pinned.hidden = false;
    if (focused) [...this.pinned.querySelectorAll<HTMLElement>("button")].find(button => button.className === focused)?.focus({ preventScroll: true });
  }

  /**
   * The operator is writing (the composer has focus on a small or touch
   * screen, or the phone keyboard is up): the pinned question folds to its
   * bar so the latest conversation stays readable above the field
   * (cas-16eed). Starting to compose again folds a question the operator had
   * opened; their own collapse stays.
   */
  setComposing(composing: boolean): void {
    if (this.disposed || this.composing === composing) return;
    this.composing = composing;
    this.renderPinned(this.element.ownerDocument);
    if (this.following) this.pin();
  }

  /** Dismiss a waiting ask: it unpins, stops waiting, and its copy in the thread keeps its choices. */
  private dismissAsk(id: number, byKeyboard: boolean): void {
    if (!this.history.dismissAsk(id)) return;
    this.update();
    this.options.dismissalsChanged?.();
    // The control that had focus is gone; the composer is where the operator goes next.
    if (byKeyboard) this.element.ownerDocument.querySelector<HTMLElement>("#message-text")?.focus({ preventScroll: true });
  }

  /** Dismiss a failed send out of the thread; the unsent chip keeps a way back. */
  private dismissSend(send: ConversationSend, bubble: HTMLElement): void {
    const hadFocus = bubble.contains(this.element.ownerDocument.activeElement);
    if (!this.history.dismissSend(send.id)) return;
    this.update();
    this.options.dismissalsChanged?.();
    if (hadFocus && !this.unsent.hidden) this.unsent.focus({ preventScroll: true });
  }

  /** Bring the dismissed failed sends back, and land on the first one's Retry. */
  private restoreUnsent(): void {
    const restored = this.history.restoreDismissed();
    if (!restored.length) return;
    this.update();
    this.options.dismissalsChanged?.();
    const key = `send:${restored[0]!.id}`;
    const first = [...this.msgs.querySelectorAll<HTMLElement>(".conversation-turn")].find((node) => node.dataset.key === key);
    if (!first) return;
    first.scrollIntoView?.({ block: "nearest" });
    landFocusIn(first, "conversation-retry");
  }

  private renderUnsent(): void {
    const dismissed = this.history.dismissedSends();
    const count = dismissed.length;
    this.unsent.hidden = count === 0;
    if (!count) { delete this.unsent.dataset.count; return; }
    // cas-b00c (journey F19): a message whose delivery was not confirmed may
    // well have arrived, so the chip does not call it unsent.
    const unconfirmed = dismissed.filter((send) => send.state === "unconfirmed").length;
    const signature = `${count}:${unconfirmed}`;
    if (this.unsent.dataset.count === signature) return;
    this.unsent.dataset.count = signature;
    // cas-ca7f (cas-b00c QA F02): the chip's tone is decided, not inherited.
    // Only messages that may well have arrived (not confirmed) take the
    // caution tone the thread gives them; any message known not to be sent
    // keeps the critical tone, because that one certainly needs the operator.
    this.unsent.dataset.tone = unconfirmed === count ? "caution" : "critical";
    const copy = unsentChipCopy(count, unconfirmed);
    const document = this.element.ownerDocument;
    const glyph = document.createElement("template"); glyph.innerHTML = WARN;
    const text = document.createElement("span"); text.textContent = copy.text;
    const action = document.createElement("span"); action.className = "conversation-unsent-show"; action.setAttribute("aria-hidden", "true"); action.textContent = "Show";
    this.unsent.replaceChildren(glyph.content.firstElementChild!, text, action);
    this.unsent.setAttribute("aria-label", copy.label);
  }

  /**
   * Who said a turn (cas-8d52, journey F13): a turn from another session
   * than the one the thread is attached to is that session's, by its own
   * codename, never the current supervisor's.
   */
  private speakerOf(session: string | undefined): string {
    return session && this.history.currentSession !== undefined && session !== this.history.currentSession ? sessionCodename(session) : this.options.supervisor;
  }

  /**
   * cas-d8a5 (journey F32): a group is spoken as "cas-src supervisor", with
   * the codename (or its earlier session) as the description.
   */
  private spokenSpeakerOf(session: string | undefined): { name: string; description?: string } {
    const codename = this.speakerOf(session);
    const other = Boolean(session && this.history.currentSession !== undefined && session !== this.history.currentSession);
    const description = supervisorDescription(this.options.project, codename, other);
    return { name: spokenSupervisor(this.options.project, codename), ...(description ? { description } : {}) };
  }

  private context(document: Document, turn: ThreadTurn, reply: OperatorReply, pinned = false): TurnRenderContext {
    const context: TurnRenderContext = {
      document, turn, reply, supervisor: this.speakerOf(turn.event.session), ...(this.options.project ? { spokenSupervisor: this.spokenSpeakerOf(turn.event.session).name } : {}),
      body: () => renderBody(document, reply, context),
      history: this.history,
      respond: this.options.respond,
      pinned,
    };
    return context;
  }

  private earlierQuestion(reply: OperatorReply): string | undefined {
    if (!reply.reply_to_session || reply.reply_to === null) return undefined;
    const entry = this.history.earlierSessions().find((entry) => entry.session === reply.reply_to_session);
    const question = entry?.events.find((event) => event.kind === "send" && event.value.notificationId === reply.reply_to);
    return question?.kind === "send" ? question.value.text : undefined;
  }

  /** An ask or blocker repaints when the operator answers it, not only when its own event changes. */
  private turnSignature(turn: ThreadTurn): string {
    const reply = turn.event.kind === "reply" ? turn.event.value : undefined;
    const answered = reply?.kind === "ask" ? this.history.answered(reply.notification_id) : undefined;
    const waiting = reply?.kind === "blocker" ? this.history.waiting().some((item) => item.notification_id === reply.notification_id) : undefined;
    // The bookmark target changes when the newest waiting question changes.
    const pinned = reply?.kind === "ask" ? this.history.pinnedAsk()?.notification_id === reply.notification_id : undefined;
    // A question that stops waiting (dismissed, or its session ended) repaints quiet (cas-16eed).
    const retired = reply?.kind === "ask" || reply?.kind === "blocker" ? this.history.retirement(reply.notification_id) : undefined;
    const delivered = turn.event.kind === "send" ? this.history.showsDelivered(turn.event.value) : undefined;
    // A refused send repaints when control changes hands (cas-8e0a).
    const held = turn.event.kind === "send" && turn.event.value.state === "error" ? this.options.controlHeld?.() === true : undefined;
    const holder = turn.event.kind === "send" && turn.event.value.state === "error" ? this.options.controlHolder?.() : undefined;
    // ...and when its session comes back (cas-d15c).
    const live = turn.event.kind === "send" && turn.event.value.state === "error" ? this.options.sessionLive?.() === true : undefined;
    // An unconfirmed send settles once the supervisor speaks after it (journey F10).
    const settled = turn.event.kind === "send" && turn.event.value.state === "unconfirmed" ? this.history.repliedSince(turn.event.value) : undefined;
    // cas-b00c: a run of unconfirmed messages repaints when Review opens or closes it.
    const review = settled === false ? [...this.reviewedRuns].join(",") : undefined;
    const earlierQuestion = reply?.reply_to_session ? this.earlierQuestion(reply) : undefined;
    // cas-97d58 F18: a live send repaints when its confirmation cue is due.
    const confirming = turn.event.kind === "send" && turn.event.value.state === "sending" ? this.history.awaitsConfirmation(turn.event.value) : undefined;
    return JSON.stringify([turn.event, earlierQuestion, answered && [answered.id, answered.state, answered.text], waiting, pinned, retired, delivered, held, holder, settled, live, review, confirming]);
  }

  /**
   * Nothing waiting (empty.html): the machine's monogram in its accent, the
   * project (then machine · codename), a quiet centred line, and the last message as a faint
   * echo. Lives beside `.msgs`, never inside it, so the log stays a log.
   */
  private renderEmpty(show: boolean, loading = false): void {
    this.empty.hidden = !show;
    this.msgs.hidden = show;
    if (!show) { this.empty.replaceChildren(); delete this.empty.dataset.signature; delete this.empty.dataset.state; return; }
    const { supervisor, machine, project } = this.options;
    // cas-010f: "no messages" only once this session's first page resolved
    // empty, and in words that match the header's connection state.
    const copy = emptyThreadCopy({ project, machine, connection: this.options.connection?.(), resolved: !loading });
    if (copy.state === "loading") {
      // Until the first page lands, "Nothing waiting" would be a guess.
      if (this.empty.dataset.state === "loading") return;
      this.empty.dataset.state = "loading";
      delete this.empty.dataset.signature;
      // cas-813a: the same line the attach showed, in the same place, with
      // its motion clock counted from when the conversation began to open.
      const since = this.options.openingSince?.();
      const delay = since === undefined ? OPENING_MOTION_DELAY_MS : OPENING_MOTION_DELAY_MS - (Date.now() - since);
      this.empty.replaceChildren(openingLine(this.element.ownerDocument, CONVERSATION_OPENING, delay));
      return;
    }
    this.empty.dataset.state = copy.state;
    const echo = this.options.echo?.()?.trim() || "";
    const activity = this.options.activity?.();
    const activityText = activity ? emptyCardActivityText(activity, Date.now()) : "";
    const signature = JSON.stringify([supervisor, machine, project, echo, activityText, copy.said]);
    if (this.empty.dataset.signature === signature) return;
    this.empty.dataset.signature = signature;
    const document = this.element.ownerDocument;
    const mono = document.createElement("span"); mono.className = "mono"; mono.setAttribute("aria-hidden", "true");
    mono.textContent = machineMonogram(machine || supervisor);
    // The project titles the card, as it titles the header and every list row
    // (journey F7); machine and codename sit beneath it. Without a project the
    // codename is the only name there is, and it keeps the title.
    const name = document.createElement("b"); name.textContent = project || supervisor; name.title = project || supervisor;
    if (!project) name.className = "codename";
    const where = document.createElement("span"); where.className = "proj2";
    where.title = [machine, project ? supervisor : undefined].filter(Boolean).join(" · ");
    // cas-766c: as in the header, the machine name holds its place; its OS
    // word goes first, then the codename yields (fitMachineLine).
    const host = document.createElement("span"); host.className = "proj2-machine"; host.innerHTML = hostMarkup(machine ?? "");
    if (machine) where.append(host);
    if (project) {
      const secondary = document.createElement("span"); secondary.className = "codename"; secondary.textContent = supervisor;
      const separator = document.createElement("span"); separator.className = "proj2-sep"; separator.textContent = " · ";
      where.append(...(machine ? [separator] : []), secondary);
    }
    // cas-55a4: an honest empty state. This session has said nothing here
    // yet; another session's thread is never shown in its place. The sentence
    // names the role (journey F13); the codename is in the line above it.
    const said = document.createElement("p"); said.className = "said"; said.setAttribute("role", "status");
    said.textContent = copy.said;
    const children: HTMLElement[] = [mono, name, where, said];
    if (activityText) {
      // One quiet line: when the session last did anything (cas-010f).
      const foot = document.createElement("p"); foot.className = "empty-foot";
      const live = document.createElement("span"); live.className = "empty-activity"; live.textContent = activityText; foot.append(live);
      children.push(foot);
    }
    if (echo) { const quiet = document.createElement("div"); quiet.className = "quiet"; quiet.textContent = echo; children.push(quiet); }
    this.empty.replaceChildren(...children);
    if (typeof requestAnimationFrame !== "undefined") requestAnimationFrame(() => this.fitEmptyMeta());
  }

  /**
   * Other sessions' Commander turns (cas-55a4): one collapsed, labelled
   * section per session, "Earlier session calm-puma-34, Yesterday". They are
   * read-only history: nothing in them waits, pins or answers. Above the
   * thread when it has turns; under the empty card when it has none, so the
   * card says first that this session has not written yet.
   */
  private renderEarlier(threadEmpty: boolean): void {
    const sessions = this.history.earlierSessions();
    this.earlier.hidden = sessions.length === 0;
    // Jump to latest is mounted outside the thread, so place by the card and the button.
    if (threadEmpty) { if (this.empty.nextElementSibling !== this.earlier) this.empty.after(this.earlier); }
    else if (this.earlier.nextElementSibling !== this.loadEarlier) this.loadEarlier.before(this.earlier);
    const now = Date.now();
    const signature = JSON.stringify(sessions.map((entry) => [entry.session, entry.lastAt, entry.events.map((event) => event.kind === "send" ? [event.value.notificationId, event.value.text] : [event.value.notification_id, event.value.message])]).concat([dayLabel(now, now)]));
    if (this.earlier.dataset.signature === signature) return;
    this.earlier.dataset.signature = signature;
    const document = this.element.ownerDocument;
    const open = new Set([...this.earlier.querySelectorAll<HTMLDetailsElement>("details[open]")].map((node) => node.dataset.session));
    this.earlier.replaceChildren(...sessions.map((entry) => earlierSessionNode(document, entry, now, open.has(entry.session))));
  }

  /** The empty card's machine · codename line, fitted like the header's (cas-71af). */
  private fitEmptyMeta(): void {
    const line = this.empty.querySelector<HTMLElement>(":scope > .proj2");
    if (!line || this.empty.hidden) return;
    const style = getComputedStyle(this.empty);
    fitMachineLine(line, this.empty.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight));
  }

  /** "the cas-src supervisor", never the generated codename (cas-71f4, journey F20). */
  private supervisorPhrase(): string {
    return this.options.project ? `the ${this.options.project} supervisor` : "the supervisor";
  }

  /** Cheap liveness poll: repaints only when the working state actually flipped. */
  refreshWorking(): void {
    if (this.disposed) return;
    // cas-71f4: a newest message still "Sending…" holds the working line back
    // (threadModel), so it is not a flip to repaint for.
    const newest = this.history.visibleEvents().at(-1);
    const sending = newest?.kind === "send" && newest.value.state === "sending" && !newest.value.held;
    const working = this.options.working?.() === true && !sending;
    if (working !== this.nodes.has("working")) this.update();
  }

  private renderItem(node: HTMLElement, item: ThreadItem): void {
    const signature = signatureOf(item, (turn) => this.turnSignature(turn));
    if (node.dataset.signature === signature) return;
    node.dataset.signature = signature;
    switch (item.type) {
      case "day": node.className = "day"; node.textContent = item.label; return;
      case "session": node.className = "session-divider"; node.textContent = item.label; return;
      case "history-end": node.className = "history-end"; node.textContent = item.label; return;
      case "working": this.renderWorking(node); return;
      case "coalesce": this.renderCoalesce(node, item); return;
      case "group": this.renderGroup(node, item); return;
    }
  }

  private renderWorking(node: HTMLElement): void {
    const document = node.ownerDocument;
    node.className = "turn working-turn";
    const line = document.createElement("div"); line.className = "working"; line.setAttribute("role", "status");
    const dots = document.createElement("span"); dots.className = "dots"; dots.setAttribute("aria-hidden", "true");
    dots.append(document.createElement("i"), document.createElement("i"), document.createElement("i"));
    line.append(dots, document.createTextNode("working"));
    node.replaceChildren(line);
  }

  /**
   * Folded statuses: a rounded box clamped at three lines. "Show full update"
   * opens the whole run — every folded update in order, latest last — and is
   * offered whenever the run hides something: earlier updates, or a latest
   * update that overflows the clamp.
   */
  private renderCoalesce(node: HTMLElement, item: ThreadCoalesce): void {
    const document = node.ownerDocument;
    const expanded = this.expanded.has(item.key);
    node.className = "turn coalesce-turn";
    const spokenStatus = this.spokenSpeakerOf(item.session);
    speaker(node, `${spokenStatus.name}, status`, item.time, item.clockAhead, spokenStatus.description);
    const line = document.createElement("div"); line.className = "coalesce";
    line.id = `coalesce-${item.key.replace(/[^\w-]/g, "-")}`;
    line.dataset.count = String(item.count);
    line.dataset.expanded = String(expanded);
    if (expanded) {
      line.append(...item.replies.map((reply) => { const p = document.createElement("p"); p.textContent = reply.message; return p; }));
    } else {
      line.textContent = coalesceText(item);
      line.title = item.replies.map((reply) => reply.message).join("\n");
    }
    const more = document.createElement("button"); more.type = "button"; more.className = "coalesce-expand";
    more.textContent = expanded ? "Show less" : "Show full update";
    more.setAttribute("aria-expanded", String(expanded));
    more.setAttribute("aria-controls", line.id);
    more.hidden = !expanded && item.count < 2;
    more.onclick = () => {
      if (this.expanded.has(item.key)) this.expanded.delete(item.key); else this.expanded.add(item.key);
      this.renderCoalesce(node, item);
      node.querySelector<HTMLButtonElement>(".coalesce-expand")?.focus();
    };
    node.replaceChildren(line, more);
    if (item.time) node.append(timeElement(document, item.time, item.clockAhead));
    // A single status can still overflow three lines at a narrow width; offer
    // the way in once layout says the clamp hid something.
    if (more.hidden && typeof requestAnimationFrame !== "undefined") requestAnimationFrame(() => syncClampPill(node));
  }

  private renderGroup(node: HTMLElement, group: ThreadGroup): void {
    const document = node.ownerDocument;
    node.className = `turn ${group.side === "you" ? "you" : "sup"}`;
    // F19 (cas-17e3): a screen reader hears who spoke and when, not bare
    // paragraphs and times. The visible time stays for sighted readers.
    const spoken = group.side === "you" ? { name: "You" } : this.spokenSpeakerOf(group.turns[0]?.event.session);
    speaker(node, spoken.name, group.time, group.clockAhead, "description" in spoken ? spoken.description : undefined);
    // Bubbles are keyed too: a later turn re-derives the earlier one's corner
    // classes without replacing its node, so a selection or focus inside it
    // survives the update.
    const existing = new Map<string, HTMLElement>();
    for (const child of node.querySelectorAll<HTMLElement>(":scope > [data-key]")) existing.set(child.dataset.key!, child);
    const children: HTMLElement[] = [];
    // cas-8e0a F01: a bubble rebuilt while focus is inside it (Take control
    // leaves a refused message once it succeeds) hands focus to its
    // replacement instead of dropping it to the page body.
    const active = document.activeElement;
    let refocus: { bubble: HTMLElement; className: string } | undefined;
    const runs = this.unconfirmedRuns(group);
    for (const [index, turn] of group.turns.entries()) {
      const run = runs.get(index);
      const signature = this.turnSignature(turn) + (run ? JSON.stringify(run) : "");
      let bubble = existing.get(turn.key);
      let sheets: HTMLElement[] = [];
      if (bubble && bubble.dataset.signature === signature) {
        for (let index = 0; ; index += 1) { const sheet = existing.get(`${turn.key}#${index}`); if (!sheet) break; sheets.push(sheet); }
      } else {
        const previous = bubble;
        const focusedClass = previous && active instanceof HTMLElement && previous.contains(active) ? active.className : undefined;
        if (turn.event.kind === "send") bubble = this.renderSend(document, turn, turn.event.value, run);
        else ({ bubble, sheets } = this.renderReply(document, turn, turn.event.value));
        bubble.classList.add("conversation-turn");
        bubble.dataset.key = turn.key;
        bubble.dataset.signature = signature;
        sheets.forEach((sheet, index) => { sheet.classList.add("conversation-sheet"); sheet.dataset.key = `${turn.key}#${index}`; });
        if (focusedClass !== undefined) refocus = { bubble, className: focusedClass };
      }
      bubble.classList.toggle("group-first", turn.first);
      bubble.classList.toggle("group-last", turn.last);
      children.push(bubble, ...sheets);
    }
    if (group.time) children.push(timeElement(document, group.time, group.clockAhead));
    node.replaceChildren(...children);
    if (refocus) landFocusIn(refocus.bubble, refocus.className);
  }

  /**
   * cas-b00c (journey F19): two or more unconfirmed messages in a row read as
   * one notice, not a stack of warning cards each with its own Retry. Maps a
   * turn's index in the group to its place in such a run.
   */
  private unconfirmedRuns(group: ThreadGroup): Map<number, UnconfirmedRun> {
    const runs = new Map<number, UnconfirmedRun>();
    const actionable = group.turns.map((turn) => turn.event.kind === "send" && turn.event.value.state === "unconfirmed" && !this.history.repliedSince(turn.event.value));
    for (let start = 0; start < actionable.length; ) {
      let end = start;
      while (end < actionable.length && actionable[end]) end += 1;
      if (end - start >= 2) {
        for (let index = start; index < end; index += 1) {
          const members = group.turns.slice(start, end).map((turn) => turn.key);
          const expanded = members.some((member) => this.reviewedRuns.has(member));
          runs.set(index, { count: end - start, last: index === end - 1, expanded, key: members[0]!, members });
        }
      }
      start = Math.max(end, start + 1);
    }
    return runs;
  }

  /** Open (or close) one unconfirmed run, and keep the keyboard user's place. */
  private toggleUnconfirmedReview(run: UnconfirmedRun, open: boolean): void {
    for (const member of run.members) if (open) this.reviewedRuns.add(member); else this.reviewedRuns.delete(member);
    this.update();
    const first = [...this.msgs.querySelectorAll<HTMLElement>(".conversation-turn")].find((node) => node.dataset.key === run.key);
    if (open) { if (first) landFocusIn(first, "conversation-retry"); return; }
    const notice = [...this.msgs.querySelectorAll<HTMLElement>(".conversation-review")].find((button) => button.dataset.run === run.key) ?? this.msgs.querySelector<HTMLElement>(".conversation-review");
    notice?.focus({ preventScroll: true });
  }

  private renderSend(document: Document, turn: ThreadTurn, send: ConversationSend, run?: UnconfirmedRun): HTMLElement {
    const bubble = document.createElement("div");
    bubble.className = "bub";
    bubble.dataset.state = send.state;
    if (send.deviceLabel) {
      const origin = document.createElement("span");
      origin.className = "conversation-send-origin";
      origin.textContent = `from ${send.deviceLabel}`;
      bubble.append(origin);
    }
    bubble.append(...paragraphs(document, send.text));
    if (send.state === "sending") {
      const state = document.createElement("span");
      state.className = `conversation-delivery${send.held ? " conversation-held" : ""}`; state.setAttribute("role", "status");
      // Held while the machine is unreachable (cas-0978): not on the wire yet,
      // and it will be sent by itself, once, when the machine is back.
      state.textContent = send.held ? "Waiting for the connection — sends when it's back"
        : this.history.awaitsConfirmation(send) ? `Waiting for ${(this.options.machine && machineName(this.options.machine)) || "the machine"} to confirm…`
          : "Sending…";
      bubble.append(state);
      if (send.held && this.options.cancelMessage) {
        const cancel = document.createElement("button");
        cancel.type = "button"; cancel.className = "conversation-edit conversation-cancel";
        cancel.textContent = "Cancel"; cancel.setAttribute("aria-label", "Cancel waiting message");
        cancel.onclick = () => this.options.cancelMessage?.(send);
        bubble.append(cancel);
      }
    } else if (this.history.showsDelivered(send)) {
      // F5: the receipt is the difference between a delivered message and a
      // lost one, so a delivered send says so until the reply linked to it
      // lands (then the answer itself is the evidence). Journey F4: an
      // unrelated supervisor turn crossing it no longer hides the tick.
      const state = document.createElement("span");
      state.className = "conversation-delivery conversation-delivered"; state.setAttribute("role", "status");
      const tick = document.createElement("template"); tick.innerHTML = TICK;
      const label = document.createElement("span"); label.textContent = "Delivered";
      state.append(tick.content.firstElementChild!, label);
      bubble.append(state);
    } else if (send.state === "unconfirmed" && run && !run.expanded) {
      // cas-b00c: one notice for the run, on its last message; the earlier
      // ones are quiet records of what was sent. Review opens each with its
      // own Retry.
      bubble.dataset.grouped = run.last ? "last" : "member";
      if (run.last) {
        const state = document.createElement("span");
        state.className = "conversation-delivery conversation-refused conversation-unconfirmed"; state.setAttribute("role", "status");
        const glyph = document.createElement("template"); glyph.innerHTML = WARN;
        const label = document.createElement("b"); label.textContent = `${run.count} messages not confirmed`;
        const separator = document.createElement("span"); separator.className = "sr-only"; separator.textContent = " · ";
        const reason = document.createElement("span"); reason.className = "conversation-refused-reason";
        reason.textContent = `Cassy couldn't confirm delivery to ${this.supervisorPhrase()}.`;
        const next = document.createElement("span"); next.className = "conversation-refused-next"; next.textContent = " Review them to retry.";
        reason.append(next);
        state.append(glyph.content.firstElementChild!, label, separator, reason);
        const actions = document.createElement("div"); actions.className = "conversation-actions";
        const review = document.createElement("button"); review.type = "button"; review.className = "conversation-review"; review.textContent = "Review";
        review.setAttribute("aria-label", `Review ${run.count} messages not confirmed`);
        review.setAttribute("aria-expanded", "false");
        review.dataset.run = run.key;
        review.onclick = () => this.toggleUnconfirmedReview(run, true);
        actions.append(review);
        bubble.append(state, actions);
      }
    } else if (send.state === "unconfirmed" && this.history.repliedSince(send)) {
      // Journey F10: the supervisor has spoken since, so this send most
      // likely arrived. The card settles to a quiet record: no warning and no
      // primary Retry inviting a duplicate. cas-470e: the copy says to send
      // it again only if it was missed, so a quiet text-weight "Send again"
      // is right there; the operator never has to retype the message.
      bubble.dataset.settled = "true";
      const state = document.createElement("span");
      state.className = "conversation-delivery conversation-refused conversation-unconfirmed conversation-settled"; state.setAttribute("role", "status");
      const label = document.createElement("b"); label.textContent = "Not confirmed";
      const separator = document.createElement("span"); separator.className = "sr-only"; separator.textContent = " · ";
      const reason = document.createElement("span"); reason.className = "conversation-refused-reason";
      reason.textContent = "The supervisor has replied since; send it again only if it missed this.";
      state.append(label, separator, reason);
      bubble.append(state);
      if (this.options.retryMessage) {
        const actions = document.createElement("div"); actions.className = "conversation-actions conversation-actions-quiet";
        const again = document.createElement("button"); again.type = "button"; again.className = "conversation-send-again"; again.textContent = "Send again";
        again.setAttribute("aria-label", "Send this message again");
        again.onclick = () => this.options.retryMessage?.(send);
        actions.append(again);
        bubble.append(actions);
      }
    } else if (send.state === "unconfirmed") {
      // cas-1622: the receipt for this send never came. It is not refused —
      // it may well have arrived — so it does not claim "Not sent". It stops
      // saying "Sending…" forever, says what is unknown in the operator's
      // words (Cassy, not "the hub": journey F10), and offers Retry, warning
      // that a retry may reach the supervisor twice.
      const state = document.createElement("span");
      state.className = "conversation-delivery conversation-refused conversation-unconfirmed"; state.setAttribute("role", "status");
      const glyph = document.createElement("template"); glyph.innerHTML = WARN;
      const label = document.createElement("b"); label.textContent = "Not confirmed";
      const separator = document.createElement("span"); separator.className = "sr-only"; separator.textContent = " · ";
      const reason = document.createElement("span"); reason.className = "conversation-refused-reason";
      // cas-71f4 (journey F20): the project's supervisor, never the codename.
      reason.textContent = `Cassy couldn't confirm delivery to ${this.supervisorPhrase()}.`;
      const next = document.createElement("span"); next.className = "conversation-refused-next"; next.textContent = " Retry sends it again.";
      reason.append(next);
      state.append(glyph.content.firstElementChild!, label, separator, reason);
      bubble.append(state);
      if (this.options.retryMessage) {
        const actions = document.createElement("div"); actions.className = "conversation-actions";
        const retry = document.createElement("button"); retry.type = "button"; retry.className = "conversation-retry"; retry.textContent = "Retry";
        retry.setAttribute("aria-label", "Retry sending");
        retry.onclick = () => this.options.retryMessage?.(send);
        actions.append(retry);
        bubble.append(actions);
      }
      if (run?.last) {
        const less = document.createElement("div"); less.className = "conversation-actions conversation-actions-quiet";
        const close = document.createElement("button"); close.type = "button"; close.className = "conversation-review-less conversation-send-again"; close.textContent = "Show less";
        close.setAttribute("aria-label", `Show ${run.count} messages not confirmed as one notice`);
        close.setAttribute("aria-expanded", "true");
        close.onclick = () => this.toggleUnconfirmedReview(run, false);
        less.append(close);
        bubble.append(less);
      }
      this.dismissable(document, bubble, send);
    } else if (send.state === "error" && send.replaced) {
      // F6: the edited version went out, so this one is only a record. It
      // collapses and offers no Retry — one tap would resend the text the
      // operator just corrected.
      bubble.dataset.replaced = "true";
      const state = document.createElement("span");
      state.className = "conversation-delivery conversation-refused conversation-replaced"; state.setAttribute("role", "status");
      const label = document.createElement("b"); label.textContent = "Not sent";
      const separator = document.createElement("span"); separator.textContent = " · ";
      const reason = document.createElement("span"); reason.className = "conversation-refused-reason"; reason.textContent = "replaced by your edit";
      state.append(label, separator, reason);
      bubble.append(state);
    } else if (send.state === "error") {
      // P8 (cas-b1ee): a refused send must not read as delivered. The bubble
      // drops its fill for a dashed critical outline; the label leads with a
      // warning glyph and "Not sent", then the refusal in plain words (F6):
      // why it did not go and the step that gets it through.
      const state = document.createElement("span");
      state.className = "conversation-delivery conversation-refused"; state.setAttribute("role", "status");
      const glyph = document.createElement("template"); glyph.innerHTML = WARN;
      const label = document.createElement("b"); label.textContent = "Not sent";
      // The separator is for the reader; on screen the reason takes its own line.
      const separator = document.createElement("span"); separator.className = "sr-only"; separator.textContent = " · ";
      const plain = refusal(send.error);
      // cas-8e0a: once this device holds control, the control refusal is
      // resolved. The message still was not sent, but Retry now goes through,
      // so the copy says so and Take control leaves the message.
      const resolved = plain.action === "take-control" && this.options.controlHeld?.() === true;
      // cas-1730: another device holds control and this one cannot take it
      // over, so the next step is to wait for its release, as the composer says.
      const holder = plain.action === "take-control" && !resolved ? this.options.controlHolder?.() : undefined;
      const reason = document.createElement("span"); reason.className = "conversation-refused-reason"; reason.textContent = resolved ? "This device controls the session now." : plain.reason;
      const next = document.createElement("span"); next.className = "conversation-refused-next";
      const sessionBack = plain.action === "await-session" && this.options.sessionLive?.() === true;
      next.textContent = resolved || sessionBack ? " Retry to send it." : holder ? ` ${holder} is in control. Take control when it's released, then retry.` : ` ${plain.next}`;
      reason.append(next);
      state.append(glyph.content.firstElementChild!, label, separator, reason);
      bubble.append(state);
      const actions = document.createElement("div"); actions.className = "conversation-actions";
      if (plain.action === "take-control" && !resolved && this.options.takeControl) {
        const take = document.createElement("button"); take.type = "button"; take.className = "conversation-take-control";
        if (holder) {
          // Journey F5: a take the hub will refuse again is not offered as
          // pressable. It says who is being waited on, keeps its place (and
          // keyboard focus) in the message, and turns back into Take control
          // when the lease is released.
          take.textContent = `Waiting for ${holder}`;
          take.setAttribute("aria-label", `Waiting for ${holder} to release control`);
          take.setAttribute("aria-disabled", "true");
          take.dataset.waiting = "true";
        } else {
          take.textContent = "Take control";
          take.setAttribute("aria-label", "Take control of the session");
          take.onclick = () => this.options.takeControl?.(send);
        }
        actions.append(take);
      }
      if (this.options.editMessage) {
        const edit = document.createElement("button"); edit.type = "button"; edit.className = "conversation-edit"; edit.textContent = "Edit";
        edit.setAttribute("aria-label", "Edit message");
        edit.onclick = () => this.options.editMessage?.(send.text, send);
        actions.append(edit);
      }
      if (this.options.retryMessage) {
        const retry = document.createElement("button"); retry.type = "button"; retry.className = "conversation-retry"; retry.textContent = "Retry";
        retry.setAttribute("aria-label", "Retry sending");
        if (holder) {
          // cas-b00c (journey F18): while another device holds control a
          // retry can only be refused again, so it is not pressable either.
          retry.setAttribute("aria-disabled", "true");
          retry.setAttribute("aria-description", `Waiting for ${holder} to release control`);
          retry.dataset.waiting = "true";
        } else {
          retry.onclick = () => this.options.retryMessage?.(send);
        }
        actions.append(retry);
      }
      if (actions.childElementCount) bubble.append(actions);
      this.dismissable(document, bubble, send);
    }
    return bubble;
  }

  /**
   * cas-16eed: a failed send can leave the thread, and the unsent chip brings
   * it back. On a touch screen it swipes off sideways; for a mouse or the
   * keyboard a Dismiss control sits in the card's corner, last in its tab
   * order, so Take control, Edit and Retry keep their row.
   */
  private dismissable(document: Document, bubble: HTMLElement, send: ConversationSend): void {
    bubble.dataset.swipe = "dismiss";
    const dismiss = document.createElement("button"); dismiss.type = "button"; dismiss.className = "conversation-dismiss";
    // cas-b00c (journey F19): an unconfirmed message may well have arrived;
    // only the notice about it is dismissed, not a message called unsent.
    dismiss.setAttribute("aria-label", send.state === "unconfirmed" ? "Dismiss this notice" : "Dismiss unsent message");
    dismiss.title = "Dismiss";
    dismiss.innerHTML = CLOSE;
    dismiss.onclick = () => this.dismissSend(send, bubble);
    bubble.append(dismiss);
    bindSwipeDismiss(bubble, { onDismiss: () => this.dismissSend(send, bubble) });
  }

  /**
   * A plain reply is a bubble; with a registered sheet renderer its artifacts
   * are laid on the thread beside it, not inside it (Pebble 4: no bubble
   * around a sheet). A custom ask/blocker object keeps its attachments inside,
   * where its own body() put them. Without a sheet renderer the link rows stay
   * in the bubble.
   */
  private renderReply(document: Document, turn: ThreadTurn, reply: OperatorReply): { bubble: HTMLElement; sheets: HTMLElement[] } {
    const kind = reply.kind ?? "answer";
    const context = this.context(document, turn, reply);
    const custom = kind === "ask" || kind === "blocker" ? turnRenderers.get(kind)?.(reply, context) : undefined;
    const sheetRenderer = turnRenderers.get("attachment");
    const sheets: HTMLElement[] = [];
    const bubble = custom ?? document.createElement("div");
    if (!custom) {
      const body = () => renderBody(document, reply, context, { attachments: sheetRenderer ? "none" : "inline" });
      bubble.className = kind === "receipt" ? "bub receipt" : "bub";
      if (kind === "receipt") {
        const tick = document.createElement("template"); tick.innerHTML = TICK;
        const text = document.createElement("div"); text.className = "receipt-text"; text.append(...body());
        bubble.append(tick.content.firstElementChild!, text);
      } else {
        bubble.append(...body());
      }
      if (sheetRenderer) for (const attachment of reply.attachments ?? []) sheets.push(sheetRenderer(reply, { ...context, attachment }));
      // An attachment-only turn is the sheet alone; an empty bubble would be a blank pebble.
      if (sheets.length && !bubble.textContent?.trim() && !bubble.querySelector(".evi")) bubble.classList.add("bub-empty");
    }
    bubble.dataset.kind = kind;
    // cas-97d58 F05 (supersedes cas-e6d2's always-visible receipt): keeping a
    // reply is the normal state, so it shows nothing and is not read out
    // after every turn; data-stored carries it for tests and tooling. Only a
    // reply this device has not kept says so, as a plain line in the card.
    if (reply.device_persisted !== undefined) {
      bubble.dataset.stored = reply.device_persisted ? "true" : "false";
      // Only a reply whose storing failed, never one still being written (a
      // line that appeared and vanished on every reply shifted the thread).
      if (!reply.device_persisted && reply.device_store_failed) {
        const receipt = document.createElement("small");
        receipt.className = "reply-storage-receipt";
        receipt.textContent = "Not kept on this device yet";
        bubble.append(receipt);
      }
    }
    bubble.dataset.replyTo = reply.reply_to === null ? "" : String(reply.reply_to);
    // cas-e829: an answer to another session's turn stays in this thread and
    // only names what it answers; the earlier session itself is read-only.
    if (reply.reply_to_session) bubble.prepend(earlierReplyQuote(document, reply.reply_to_session, this.earlierQuestion(reply)));
    return { bubble, sheets };
  }

  /**
   * Scroll to the tail and follow it again: the composer took focus (the phone
   * keyboard is coming up), so the latest turn and the pinned ask must be the
   * thing above the field, whatever the operator had scrolled to before.
   */
  followTail(): void {
    if (this.disposed) return;
    this.following = true;
    this.pin();
  }

  private pin(): void {
    this.element.scrollTop = this.element.scrollHeight;
    this.scrolledTo = this.element.scrollTop;
    this.lastContentHeight = this.element.scrollHeight;
    this.jump.hidden = true;
    if (this.pinPending) return;
    this.pinPending = true;
    requestAnimationFrame(() => {
      if (this.disposed) return;
      // cas-c2cb: a reader who moved focus off the tail meanwhile stopped
      // following it; the frame-late re-pin leaves them where they went.
      if (this.following) this.element.scrollTop = this.element.scrollHeight;
      requestAnimationFrame(() => { this.pinPending = false; });
    });
  }

  dispose(): void { this.disposed = true; this.resize?.disconnect(); this.element.remove(); this.pinned.remove(); this.unsent.remove(); this.jump.remove(); }
}

/** Show the expand pill on a lone folded status only while the three-line clamp is hiding text. */
function syncClampPill(node: HTMLElement): void {
  const line = node.querySelector<HTMLElement>(":scope > .coalesce");
  const more = node.querySelector<HTMLButtonElement>(":scope > .coalesce-expand");
  if (!line || !more || !line.isConnected || line.dataset.expanded === "true" || Number(line.dataset.count) > 1) return;
  more.hidden = !(line.scrollHeight > line.clientHeight + 1);
}

function signatureOf(item: ThreadItem, turnSignature: (turn: ThreadTurn) => string): string {
  switch (item.type) {
    case "day": return `day:${item.label}`;
    case "session": return `session:${item.label}`;
    case "history-end": return "history-end";
    case "working": return "working";
    case "coalesce": return JSON.stringify([item.count, item.latest, item.time, item.clockAhead === true, item.replies.map((reply) => reply.notification_id)]);
    case "group": return JSON.stringify([item.side, item.time, item.clockAhead === true, item.turns.map((turn) => [turn.key, turn.first, turn.last, turnSignature(turn)])]);
  }
}

/** Names a message group for assistive tech: "You, 12:45" or "<supervisor>, 12:45". */
function speaker(node: HTMLElement, who: string, time: string | undefined, clockAhead = false, description?: string): void {
  node.setAttribute("role", "group");
  node.setAttribute("aria-label", joinSpoken([who, time, clockAhead && CLOCK_AHEAD]));
  if (description) node.setAttribute("aria-description", description); else node.removeAttribute("aria-description");
}

/** The quiet hint beside a time that is the arrival time, not the machine's own stamp (cas-1f13). */
export const CLOCK_AHEAD = "machine clock ahead";
const CLOCK_AHEAD_TITLE = "This machine's clock is ahead of yours, so this shows when the message arrived.";

/** A group's one visible time, with the clock-ahead hint when it applies. */
function timeElement(document: Document, time: string, clockAhead: boolean | undefined): HTMLElement {
  const element = document.createElement("time"); element.textContent = time; element.setAttribute("aria-hidden", "true");
  if (clockAhead) {
    element.title = CLOCK_AHEAD_TITLE;
    const hint = document.createElement("span"); hint.className = "clock-ahead"; hint.textContent = ` · ${CLOCK_AHEAD}`;
    element.append(hint);
  }
  return element;
}

function paragraphs(document: Document, text: string): HTMLElement[] {
  return text.split(/\n{2,}/).map((chunk) => chunk.trim()).filter(Boolean).map((chunk) => {
    const p = document.createElement("p"); p.textContent = chunk; return p;
  });
}

/**
 * Prose paragraphs plus evidence tables. Attachments follow: as sheets when a
 * renderer is registered, as link rows otherwise, or not at all when the
 * caller lays the sheets beside the bubble itself (`attachments: "none"`).
 */
export function renderBody(document: Document, reply: OperatorReply, context?: TurnRenderContext, options: { attachments?: "inline" | "none" } = {}): HTMLElement[] {
  const nodes: HTMLElement[] = [];
  for (const block of messageBlocks(reply.message)) {
    if (block.type === "text") { nodes.push(...renderMarkdown(document, block.text)); continue; }
    const table = document.createElement("div"); table.className = "evi"; table.setAttribute("role", "table");
    const columns = Math.max(block.table.header?.length ?? 0, ...block.table.rows.map((row) => row.length));
    table.dataset.columns = String(columns);
    const row = (cells: string[], head: boolean): HTMLElement => {
      const line = document.createElement("div"); line.className = head ? "evi-row evi-head" : "evi-row"; line.setAttribute("role", "row");
      // First column takes the slack, the rest hug their numbers; widths beyond three columns are set here, not in CSS.
      if (columns !== 3) line.style.gridTemplateColumns = `minmax(0, 1fr)${" auto".repeat(Math.max(0, columns - 1))}`;
      for (let index = 0; index < columns; index += 1) {
        const cell = document.createElement("span"); cell.setAttribute("role", head ? "columnheader" : "cell");
        const text = cells[index] ?? ""; cell.textContent = text;
        const tone = head ? undefined : cellTone(text); if (tone) cell.classList.add(tone);
        line.append(cell);
      }
      return line;
    };
    if (block.table.header) table.append(row(block.table.header, true));
    for (const cells of block.table.rows) table.append(row(cells, false));
    nodes.push(table);
  }
  if (options.attachments === "none") return nodes;
  const sheet = turnRenderers.get("attachment");
  for (const attachment of reply.attachments ?? []) {
    if (sheet && context) { nodes.push(sheet(reply, { ...context, attachment })); continue; }
    const row = document.createElement("div"); row.className = "conversation-attachment";
    const link = document.createElement("a"); link.href = `#artifact:${encodeURIComponent(attachment.artifact_id)}`; link.dataset.artifactId = attachment.artifact_id; link.textContent = attachment.name; link.title = `${attachment.mime} · ${attachment.size_bytes} bytes`;
    row.append(link); nodes.push(row);
  }
  return nodes;
}

export type { ConversationEvent };
