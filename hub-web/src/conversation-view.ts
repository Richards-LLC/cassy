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
  /** Refused sends offer to put their text back into the composer. */
  editMessage?: (text: string) => void;
}

const TICK = '<svg class="tick" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2.6 8.6l3.3 3.3L13.4 4.4"/></svg>';

export class ConversationView {
  readonly element: HTMLElement;
  private readonly head: HTMLElement;
  private readonly msgs: HTMLElement;
  private readonly jump: HTMLButtonElement;
  private readonly options: ConversationViewOptions;
  private nodes = new Map<string, HTMLElement>();
  private following = true;
  private pinPending = false;
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
    const name = document.createElement("b"); name.textContent = supervisor;
    const where = document.createElement("span");
    where.textContent = [this.options.machine, this.options.project].filter(Boolean).join(" · ");
    identity.append(name, where);
    this.head.append(identity);
    this.msgs = document.createElement("div"); this.msgs.className = "msgs";
    this.msgs.setAttribute("role", "log");
    this.jump = document.createElement("button"); this.jump.type = "button";
    this.jump.className = "conversation-jump"; this.jump.textContent = "Jump to latest"; this.jump.hidden = true;
    this.jump.onclick = () => { this.following = true; this.update(); this.pin(); };
    this.element.append(this.head, this.msgs, this.jump);
    this.element.addEventListener("scroll", () => {
      if (this.pinPending) return;
      this.following = shouldFollowTail(this.element);
      this.jump.hidden = this.following;
    }, { passive: true });
    if (typeof ResizeObserver !== "undefined") {
      this.resize = new ResizeObserver(() => { if (this.following) this.pin(); });
      this.resize.observe(this.element);
    }
  }

  /** Re-derive the thread from the history; nodes are keyed so grouping survives. */
  update(): void {
    if (this.disposed) return;
    const working = this.options.working?.() === true;
    const model = threadModel(this.history.events, { working });
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
    // Only re-append when the sequence changed: an unchanged list keeps its
    // scroll position and selection.
    const same = this.msgs.children.length === children.length && children.every((node, index) => this.msgs.children[index] === node);
    if (!same) this.msgs.replaceChildren(...children);
    if (this.following && document.getSelection()?.isCollapsed !== false) this.pin();
  }

  /** Cheap liveness poll: repaints only when the working state actually flipped. */
  refreshWorking(): void {
    if (this.disposed) return;
    const working = this.options.working?.() === true;
    if (working !== this.nodes.has("working")) this.update();
  }

  private renderItem(node: HTMLElement, item: ThreadItem): void {
    const signature = signatureOf(item);
    if (node.dataset.signature === signature) return;
    node.dataset.signature = signature;
    switch (item.type) {
      case "day": node.className = "day"; node.textContent = item.label; return;
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

  private renderCoalesce(node: HTMLElement, item: ThreadCoalesce): void {
    const document = node.ownerDocument;
    node.className = "turn coalesce-turn";
    const line = document.createElement("div"); line.className = "coalesce";
    line.dataset.count = String(item.count);
    line.textContent = coalesceText(item);
    line.title = item.replies.map((reply) => reply.message).join("\n");
    node.replaceChildren(line);
    if (item.time) { const time = document.createElement("time"); time.textContent = item.time; node.append(time); }
  }

  private renderGroup(node: HTMLElement, group: ThreadGroup): void {
    const document = node.ownerDocument;
    node.className = `turn ${group.side === "you" ? "you" : "sup"}`;
    // Bubbles are keyed too: a later turn re-derives the earlier one's corner
    // classes without replacing its node, so a selection or focus inside it
    // survives the update.
    const existing = new Map<string, HTMLElement>();
    for (const child of node.querySelectorAll<HTMLElement>(":scope > [data-key]")) existing.set(child.dataset.key!, child);
    const children: HTMLElement[] = [];
    for (const turn of group.turns) {
      const signature = JSON.stringify(turn.event);
      let bubble = existing.get(turn.key);
      if (!bubble || bubble.dataset.signature !== signature) {
        bubble = turn.event.kind === "send" ? this.renderSend(document, turn, turn.event.value) : this.renderReply(document, turn, turn.event.value);
        bubble.classList.add("conversation-turn");
        bubble.dataset.key = turn.key;
        bubble.dataset.signature = signature;
      }
      bubble.classList.toggle("group-first", turn.first);
      bubble.classList.toggle("group-last", turn.last);
      children.push(bubble);
    }
    if (group.time) { const time = document.createElement("time"); time.textContent = group.time; children.push(time as unknown as HTMLElement); }
    node.replaceChildren(...children);
  }

  private renderSend(document: Document, turn: ThreadTurn, send: ConversationSend): HTMLElement {
    const bubble = document.createElement("div");
    bubble.className = "bub";
    bubble.dataset.state = send.state;
    bubble.append(...paragraphs(document, send.text));
    if (send.state === "sending" || send.state === "error") {
      const state = document.createElement("span");
      state.className = "conversation-delivery"; state.setAttribute("role", "status");
      state.textContent = send.state === "sending" ? "Sending…" : `Not sent · ${send.error ?? "refused"}`;
      bubble.append(state);
      if (send.state === "error" && this.options.editMessage) {
        const edit = document.createElement("button"); edit.type = "button"; edit.className = "conversation-edit"; edit.textContent = "Edit message";
        edit.onclick = () => this.options.editMessage?.(send.text);
        bubble.append(edit);
      }
    }
    return bubble;
  }

  private renderReply(document: Document, turn: ThreadTurn, reply: OperatorReply): HTMLElement {
    const kind = reply.kind ?? "answer";
    const context: TurnRenderContext = { document, turn, reply, supervisor: this.options.supervisor, body: () => renderBody(document, reply, context) };
    const custom = kind === "ask" || kind === "blocker" ? turnRenderers.get(kind)?.(reply, context) : undefined;
    const body = context.body;
    const bubble = custom ?? document.createElement("div");
    if (!custom) {
      bubble.className = kind === "receipt" ? "bub receipt" : "bub";
      if (kind === "receipt") {
        const tick = document.createElement("template"); tick.innerHTML = TICK;
        const text = document.createElement("div"); text.className = "receipt-text"; text.append(...body());
        bubble.append(tick.content.firstElementChild!, text);
      } else {
        bubble.append(...body());
      }
    }
    bubble.dataset.kind = kind;
    bubble.dataset.replyTo = reply.reply_to === null ? "" : String(reply.reply_to);
    return bubble;
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

  dispose(): void { this.disposed = true; this.resize?.disconnect(); this.element.remove(); }
}

function signatureOf(item: ThreadItem): string {
  switch (item.type) {
    case "day": return `day:${item.label}`;
    case "working": return "working";
    case "coalesce": return JSON.stringify([item.count, item.latest, item.time, item.replies.map((reply) => reply.notification_id)]);
    case "group": return JSON.stringify([item.side, item.time, item.turns.map((turn) => [turn.key, turn.first, turn.last, turn.event])]);
  }
}

function paragraphs(document: Document, text: string): HTMLElement[] {
  return text.split(/\n{2,}/).map((chunk) => chunk.trim()).filter(Boolean).map((chunk) => {
    const p = document.createElement("p"); p.textContent = chunk; return p;
  });
}

/** Prose paragraphs plus evidence tables; attachments keep their link rows for Pebble 4. */
export function renderBody(document: Document, reply: OperatorReply, context?: TurnRenderContext): HTMLElement[] {
  const nodes: HTMLElement[] = [];
  for (const block of messageBlocks(reply.message)) {
    if (block.type === "text") { nodes.push(...paragraphs(document, block.text)); continue; }
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
