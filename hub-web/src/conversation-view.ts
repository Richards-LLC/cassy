import { transcriptLines, shouldFollowTail } from "./transcript";
import type { TranscriptSource } from "./transcript-view";
import type { ConversationEvent, ConversationHistory } from "./conversation-history";

/** The pane is one explicitly live document; channel events are actual turns.
 * Move that document when its text changes, without duplicating TUI redraws or
 * claiming that a live terminal snapshot supplies durable chat boundaries. */
export class ConversationView {
  readonly element: HTMLElement;
  private readonly flow: HTMLElement;
  private readonly pane: HTMLElement;
  private readonly paneBody: HTMLElement;
  private readonly jump: HTMLButtonElement;
  private paneText: string | undefined;
  private eventNodes = new Map<string, HTMLElement>();
  private following = true;
  private pinPending = false;
  private disposed = false;
  private resize?: ResizeObserver;
  constructor(document: Document, private source: TranscriptSource, private history: ConversationHistory, private supervisor: string) {
    this.element = document.createElement("div");
    this.element.className = "conversation-reading";
    this.element.tabIndex = 0;
    this.element.setAttribute("aria-label", `Conversation with ${supervisor}`);
    this.flow = document.createElement("div"); this.flow.className = "conversation-flow";
    this.pane = document.createElement("article"); this.pane.className = "conversation-pane";
    const heading = document.createElement("header");
    const name = document.createElement("strong"); name.textContent = supervisor;
    const label = document.createElement("span"); label.textContent = "Live pane text";
    heading.append(name, label);
    this.paneBody = document.createElement("div"); this.paneBody.className = "conversation-pane-text";
    this.pane.append(heading, this.paneBody);
    this.jump = document.createElement("button"); this.jump.type = "button";
    this.jump.className = "conversation-jump"; this.jump.textContent = "Jump to latest"; this.jump.hidden = true;
    this.jump.onclick = () => { this.following = true; this.source.scrollToBottom(); this.update(); this.pin(); };
    this.element.append(this.flow, this.jump);
    this.element.addEventListener("scroll", () => {
      if (this.pinPending) return;
      this.following = shouldFollowTail(this.element);
      this.jump.hidden = this.following;
      if (this.element.scrollTop === 0 && this.source.hasScrollbackAbove()) this.source.scrollRows(-5);
    }, { passive: true });
    if (typeof ResizeObserver !== "undefined") {
      this.resize = new ResizeObserver(() => { if (this.following) this.pin(); });
      this.resize.observe(this.element);
    }
    // Selecting text never opens a keyboard or sends raw terminal input.
  }
  update(): void {
    const lines = transcriptLines(this.source.rows(), this.source.theme());
    const text = lines.map((line) => line.text).join("\n");
    const changed = text !== this.paneText;
    if (changed) {
      this.paneText = text;
      let fenced = false;
      this.paneBody.replaceChildren(...lines.map((line) => {
        const node = this.element.ownerDocument.createElement("p");
        if (line.text.trimStart().startsWith("```")) fenced = !fenced;
        const code = fenced || line.text.trimStart().startsWith("```") || /[\u2500-\u257f]/u.test(line.text) || /^\s{4}/.test(line.text);
        node.className = code ? "conversation-code" : "conversation-line";
        if (code) { node.tabIndex = 0; node.setAttribute("aria-label", "Code or diagram; scroll horizontally to read"); }
        node.textContent = line.text || "\u00a0";
        return node;
      }));
    }
    if (!this.pane.isConnected && text.trim()) this.flow.append(this.pane);
    for (const event of this.history.events) {
      const id = event.kind === "send" ? `send:${event.value.id}` : `reply:${event.value.notification_id}`;
      let node = this.eventNodes.get(id);
      if (!node) { node = this.element.ownerDocument.createElement("article"); this.eventNodes.set(id, node); this.flow.append(node); }
      this.renderEvent(node, event);
    }
    // Only an actual text change advances the live pane, never a heartbeat.
    if (changed && text.trim() && this.flow.lastElementChild !== this.pane) this.flow.append(this.pane);
    if (this.following && this.element.ownerDocument.getSelection()?.isCollapsed !== false) this.pin();
  }
  private renderEvent(node: HTMLElement, event: ConversationEvent): void {
    const document = node.ownerDocument;
    const signature = JSON.stringify(event);
    if (node.dataset.signature === signature) return;
    node.dataset.signature = signature;
    node.className = `conversation-turn ${event.kind === "send" ? "from-you" : "from-supervisor"}`;
    const header = document.createElement("header");
    const sender = document.createElement("strong"); sender.textContent = event.kind === "send" ? "You" : this.supervisor;
    const state = document.createElement("span");
    state.className = "conversation-delivery"; state.setAttribute("role", "status");
    if (event.kind === "send") {
      const send = event.value;
      node.dataset.state = send.state;
      state.textContent = send.state === "sending" ? "Sending · awaiting receipt" : send.state === "error" ? `Not sent · ${send.error}` : send.state === "replied" ? "Replied" : send.stamped ? "Acknowledged · sent from you" : "Acknowledged";
    } else { node.dataset.replyTo = String(event.value.reply_to); state.textContent = "Reply to you"; }
    const body = document.createElement("p"); body.textContent = event.kind === "send" ? event.value.text : event.value.message;
    header.append(sender, state); node.replaceChildren(header, body);
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
