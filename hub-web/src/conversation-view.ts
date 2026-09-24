import { machineMonogram } from "./machine-accent";
import { renderMarkdown } from "./markdown-renderer";
import { refusal } from "./refusal";
import { shouldFollowTail } from "./transcript";
import type { ConversationEvent, ConversationHistory, ConversationSend } from "./conversation-history";
import type { ArtifactRef, OperatorReply, OperatorTurnKind } from "./types";
import {
  cellTone,
  coalesceText,
  messageBlocks,
  threadModel,
  type ThreadCoalesce,
  type ThreadGroup,
  type ThreadItem,
  type ThreadTurn,
} from "./thread-model";

/**
 * Pebble thread (cas-d167). The default conversation shows only operator
 * turns and supervisor→operator turns; the live pane never appears here — the
 * terminal stays the explicit alternate view.
 */

/** What a kind-specific renderer receives. */
export interface TurnRenderContext {
  readonly document: Document;
  readonly turn: ThreadTurn;
  readonly reply: OperatorReply;
  readonly supervisor: string;
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
  /**
   * A send refused because this device does not control the session offers
   * Take control on the message itself, beside Retry (cas-3433): the refusal
   * tells the operator to take control, so the control sits where they read it.
   */
  takeControl?: (send: ConversationSend) => void;
  /**
   * Quick replies and composer replies to an ask go through this; the caller
   * sends with in_reply_to = the ask's notification_id and records the send
   * in the history with the same replyTo, which is what marks the ask answered.
   */
  respond?: (ask: OperatorReply, text: string) => void;
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
  /** The loaded page reaches the beginning of the project history. */
  historyEnd?: () => boolean;
}

const TICK = '<svg class="tick" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.6 8.6l3.3 3.3L13.4 4.4"/></svg>';
/** Warning triangle for a refused send; decorative — the "Not sent" text carries the meaning. */
const WARN = '<svg class="warn" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 1.9 14.6 13.6H1.4Z"/><path d="M8 6.2v3.4"/><path d="M8 11.7v.1"/></svg>';

export class ConversationView {
  readonly element: HTMLElement;
  /**
   * The unanswered ask, pinned directly above the composer as well as in the
   * flow (Pebble 3). Mount it beside the composer; it hides itself when no ask
   * is waiting and unpins on answer.
   */
  readonly pinned: HTMLElement;
  private readonly head: HTMLElement;
  private readonly loadEarlier: HTMLButtonElement;
  private readonly msgs: HTMLElement;
  private readonly empty: HTMLElement;
  /**
   * "Jump to latest", shown while the reader is scrolled away from the tail.
   * Mount it above the composer (as main.ts does), outside the scrolling
   * thread, so it takes its own row instead of floating over a turn (cas-97ea).
   */
  readonly jump: HTMLButtonElement;
  private readonly options: ConversationViewOptions;
  private nodes = new Map<string, HTMLElement>();
  /** Coalesced status lines the operator opened with "Show full update"; survives repaints. */
  private expanded = new Set<string>();
  private following = true;
  private pinPending = false;
  private disposed = false;
  private resize?: ResizeObserver;

  constructor(document: Document, private history: ConversationHistory, options: ConversationViewOptions | string, editMessage?: (text: string) => void) {
    this.options = typeof options === "string" ? { supervisor: options, editMessage } : options;
    const { supervisor } = this.options;
    this.element = document.createElement("div");
    this.element.className = "conversation-reading thread";
    // Kept in place when a terminal surface mounts beneath it (cas-04ee).
    this.element.dataset.mountOverlay = "";
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
    this.loadEarlier.onclick = () => this.options.loadEarlier?.();
    this.msgs = document.createElement("div"); this.msgs.className = "msgs";
    this.msgs.setAttribute("role", "log");
    this.empty = document.createElement("div"); this.empty.className = "empty"; this.empty.hidden = true;
    this.jump = document.createElement("button"); this.jump.type = "button";
    this.jump.className = "conversation-jump"; this.jump.textContent = "Jump to latest"; this.jump.hidden = true;
    this.jump.onclick = () => { this.following = true; this.update(); this.pin(); };
    this.element.append(...(this.options.header === false ? [] : [this.head]), this.loadEarlier, this.msgs, this.empty, this.jump);
    this.pinned = document.createElement("div"); this.pinned.className = "pinned-ask"; this.pinned.hidden = true;
    if (this.options.accentClass) this.pinned.classList.add(this.options.accentClass);
    this.pinned.setAttribute("role", "region"); this.pinned.setAttribute("aria-label", `Waiting on you: question from ${supervisor}`);
    this.element.addEventListener("scroll", () => {
      if (this.pinPending) return;
      this.following = shouldFollowTail(this.element);
      this.jump.hidden = this.following;
    }, { passive: true });
    if (typeof ResizeObserver !== "undefined") {
      this.resize = new ResizeObserver(() => {
        for (const node of this.msgs.querySelectorAll<HTMLElement>(".coalesce-turn")) syncClampPill(node);
        if (this.following) this.pin();
      });
      this.resize.observe(this.element);
    }
  }

  /** Re-derive the thread from the history; nodes are keyed so grouping survives. */
  update(): void {
    if (this.disposed) return;
    const hasEarlier = this.options.hasEarlier?.() === true;
    const loadingEarlier = this.options.loadingEarlier?.() === true;
    this.loadEarlier.hidden = !hasEarlier;
    this.renderLoadEarlier(loadingEarlier);
    const working = this.options.working?.() === true;
    const model = threadModel(this.history.events, { working, historyEnd: this.options.historyEnd?.() === true });
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
    this.renderEmpty(model.length === 0, this.options.loadingHistory?.() === true);
    if (this.following && document.getSelection()?.isCollapsed !== false) this.pin();
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

  /** The most recent unanswered ask, pinned above the composer; answering unpins it. */
  private renderPinned(document: Document): void {
    const ask = this.history.pinnedAsk();
    const render = ask && turnRenderers.get("ask");
    if (!ask || !render) {
      this.pinned.hidden = true; this.pinned.replaceChildren(); delete this.pinned.dataset.signature;
      return;
    }
    const signature = JSON.stringify([ask.notification_id, ask.message, ask.options]);
    if (!this.pinned.hidden && this.pinned.dataset.signature === signature) return;
    this.pinned.dataset.signature = signature;
    const turn: ThreadTurn = { key: `reply:${ask.notification_id}`, side: "supervisor", kind: "ask", event: { kind: "reply", value: ask }, first: true, last: true };
    const context = this.context(document, turn, ask, true);
    const object = render(ask, context);
    object.dataset.kind = "ask"; object.dataset.pinned = "true";
    const label = document.createElement("span"); label.className = "pinned-label"; label.textContent = "Waiting on you";
    this.pinned.replaceChildren(label, object);
    this.pinned.hidden = false;
  }

  private context(document: Document, turn: ThreadTurn, reply: OperatorReply, pinned = false): TurnRenderContext {
    const context: TurnRenderContext = {
      document, turn, reply, supervisor: this.options.supervisor,
      body: () => renderBody(document, reply, context),
      history: this.history,
      respond: this.options.respond,
      pinned,
    };
    return context;
  }

  /** An ask or blocker repaints when the operator answers it, not only when its own event changes. */
  private turnSignature(turn: ThreadTurn): string {
    const reply = turn.event.kind === "reply" ? turn.event.value : undefined;
    const answered = reply?.kind === "ask" ? this.history.answered(reply.notification_id) : undefined;
    const waiting = reply?.kind === "blocker" ? this.history.waiting().some((item) => item.notification_id === reply.notification_id) : undefined;
    // The pinned ask's flow copy is collapsed; it expands again when a newer ask takes the pin.
    const pinned = reply?.kind === "ask" ? this.history.pinnedAsk()?.notification_id === reply.notification_id : undefined;
    const delivered = turn.event.kind === "send" ? this.history.delivered() === turn.event.value : undefined;
    return JSON.stringify([turn.event, answered && [answered.id, answered.state, answered.text], waiting, pinned, delivered]);
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
    if (loading) {
      // Until the first page lands, "Nothing waiting" would be a guess.
      if (this.empty.dataset.state === "loading") return;
      this.empty.dataset.state = "loading";
      delete this.empty.dataset.signature;
      const document = this.element.ownerDocument;
      const line = document.createElement("p"); line.className = "said conversation-loading"; line.setAttribute("role", "status");
      const dots = document.createElement("span"); dots.className = "dots"; dots.setAttribute("aria-hidden", "true");
      dots.append(document.createElement("i"), document.createElement("i"), document.createElement("i"));
      const codename = document.createElement("span"); codename.className = "codename"; codename.textContent = supervisor;
      const text = document.createElement("span"); text.append("Loading your conversation with ", codename, "…");
      line.append(dots, text);
      this.empty.replaceChildren(line);
      return;
    }
    delete this.empty.dataset.state;
    const echo = this.options.echo?.()?.trim() || "";
    const signature = JSON.stringify([supervisor, machine, project, echo]);
    if (this.empty.dataset.signature === signature) return;
    this.empty.dataset.signature = signature;
    const document = this.element.ownerDocument;
    const mono = document.createElement("span"); mono.className = "mono"; mono.setAttribute("aria-hidden", "true");
    mono.textContent = machineMonogram(machine || supervisor);
    // The project titles the card, as it titles the header and every list row
    // (journey F7); machine and codename sit beneath it. Without a project the
    // codename is the only name there is, and it keeps the title.
    const name = document.createElement("b"); name.textContent = project || supervisor;
    if (!project) name.className = "codename";
    const where = document.createElement("span"); where.className = "proj2";
    if (machine) where.append(machine);
    if (project) {
      const secondary = document.createElement("span"); secondary.className = "codename"; secondary.textContent = supervisor;
      where.append(...(machine ? [" · "] : []), secondary);
    }
    const said = document.createElement("p"); said.className = "said"; said.setAttribute("role", "status");
    // The codename is an identifier: mono and never broken at its hyphen, even inside prose.
    const codename = document.createElement("span"); codename.className = "codename"; codename.textContent = supervisor;
    said.append("Nothing waiting on you. ", codename, " will write here when it needs a decision.");
    const children: HTMLElement[] = [mono, name, where, said];
    if (echo) { const quiet = document.createElement("div"); quiet.className = "quiet"; quiet.textContent = echo; children.push(quiet); }
    this.empty.replaceChildren(...children);
  }

  /** Cheap liveness poll: repaints only when the working state actually flipped. */
  refreshWorking(): void {
    if (this.disposed) return;
    const working = this.options.working?.() === true;
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
    speaker(node, `${this.options.supervisor}, status`, item.time);
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
    if (item.time) { const time = document.createElement("time"); time.textContent = item.time; time.setAttribute("aria-hidden", "true"); node.append(time); }
    // A single status can still overflow three lines at a narrow width; offer
    // the way in once layout says the clamp hid something.
    if (more.hidden && typeof requestAnimationFrame !== "undefined") requestAnimationFrame(() => syncClampPill(node));
  }

  private renderGroup(node: HTMLElement, group: ThreadGroup): void {
    const document = node.ownerDocument;
    node.className = `turn ${group.side === "you" ? "you" : "sup"}`;
    // F19 (cas-17e3): a screen reader hears who spoke and when, not bare
    // paragraphs and times. The visible time stays for sighted readers.
    speaker(node, group.side === "you" ? "You" : this.options.supervisor, group.time);
    // Bubbles are keyed too: a later turn re-derives the earlier one's corner
    // classes without replacing its node, so a selection or focus inside it
    // survives the update.
    const existing = new Map<string, HTMLElement>();
    for (const child of node.querySelectorAll<HTMLElement>(":scope > [data-key]")) existing.set(child.dataset.key!, child);
    const children: HTMLElement[] = [];
    for (const turn of group.turns) {
      const signature = this.turnSignature(turn);
      let bubble = existing.get(turn.key);
      let sheets: HTMLElement[] = [];
      if (bubble && bubble.dataset.signature === signature) {
        for (let index = 0; ; index += 1) { const sheet = existing.get(`${turn.key}#${index}`); if (!sheet) break; sheets.push(sheet); }
      } else {
        if (turn.event.kind === "send") bubble = this.renderSend(document, turn, turn.event.value);
        else ({ bubble, sheets } = this.renderReply(document, turn, turn.event.value));
        bubble.classList.add("conversation-turn");
        bubble.dataset.key = turn.key;
        bubble.dataset.signature = signature;
        sheets.forEach((sheet, index) => { sheet.classList.add("conversation-sheet"); sheet.dataset.key = `${turn.key}#${index}`; });
      }
      bubble.classList.toggle("group-first", turn.first);
      bubble.classList.toggle("group-last", turn.last);
      children.push(bubble, ...sheets);
    }
    if (group.time) { const time = document.createElement("time"); time.textContent = group.time; time.setAttribute("aria-hidden", "true"); children.push(time as unknown as HTMLElement); }
    node.replaceChildren(...children);
  }

  private renderSend(document: Document, turn: ThreadTurn, send: ConversationSend): HTMLElement {
    const bubble = document.createElement("div");
    bubble.className = "bub";
    bubble.dataset.state = send.state;
    bubble.append(...paragraphs(document, send.text));
    if (send.state === "sending") {
      const state = document.createElement("span");
      state.className = "conversation-delivery"; state.setAttribute("role", "status");
      state.textContent = "Sending…";
      bubble.append(state);
    } else if (send.state === "acknowledged" && this.history.delivered() === send) {
      // F5: the hub's receipt is the difference between a delivered message
      // and a lost one, so the latest delivered send says so until the reply
      // lands (then the answer itself is the evidence).
      const state = document.createElement("span");
      state.className = "conversation-delivery conversation-delivered"; state.setAttribute("role", "status");
      const tick = document.createElement("template"); tick.innerHTML = TICK;
      const label = document.createElement("span"); label.textContent = "Delivered";
      state.append(tick.content.firstElementChild!, label);
      bubble.append(state);
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
      const reason = document.createElement("span"); reason.className = "conversation-refused-reason"; reason.textContent = plain.reason;
      const next = document.createElement("span"); next.className = "conversation-refused-next"; next.textContent = ` ${plain.next}`;
      reason.append(next);
      state.append(glyph.content.firstElementChild!, label, separator, reason);
      bubble.append(state);
      const actions = document.createElement("div"); actions.className = "conversation-actions";
      if (plain.action === "take-control" && this.options.takeControl) {
        const take = document.createElement("button"); take.type = "button"; take.className = "conversation-take-control"; take.textContent = "Take control";
        take.setAttribute("aria-label", "Take control of the session");
        take.onclick = () => this.options.takeControl?.(send);
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
        retry.onclick = () => this.options.retryMessage?.(send);
        actions.append(retry);
      }
      if (actions.childElementCount) bubble.append(actions);
    }
    return bubble;
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
    bubble.dataset.replyTo = reply.reply_to === null ? "" : String(reply.reply_to);
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
    this.jump.hidden = true;
    if (this.pinPending) return;
    this.pinPending = true;
    requestAnimationFrame(() => {
      if (this.disposed) return;
      this.element.scrollTop = this.element.scrollHeight;
      requestAnimationFrame(() => { this.pinPending = false; });
    });
  }

  dispose(): void { this.disposed = true; this.resize?.disconnect(); this.element.remove(); this.pinned.remove(); }
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
    case "coalesce": return JSON.stringify([item.count, item.latest, item.time, item.replies.map((reply) => reply.notification_id)]);
    case "group": return JSON.stringify([item.side, item.time, item.turns.map((turn) => [turn.key, turn.first, turn.last, turnSignature(turn)])]);
  }
}

/** Names a message group for assistive tech: "You, 12:45" or "<supervisor>, 12:45". */
function speaker(node: HTMLElement, who: string, time: string | undefined): void {
  node.setAttribute("role", "group");
  node.setAttribute("aria-label", time ? `${who}, ${time}` : who);
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
