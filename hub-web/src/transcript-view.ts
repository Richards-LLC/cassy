import type { GhosttyRow } from "./terminal/ghostty/core";
import {
  shouldFollowTail,
  shouldPageScrollback,
  transcriptLineKey,
  transcriptLines,
  type TranscriptLine,
  type TranscriptTheme,
} from "./transcript";

/**
 * The pane's grid, as the transcript needs to see it. Implemented by the
 * Ghostty surface; kept as an interface so the reading view can be driven from
 * a test or a fixture without a WASM terminal.
 */
export interface TranscriptSource {
  rows(): readonly GhosttyRow[];
  theme(): TranscriptTheme;
  hasScrollbackAbove(): boolean;
  scrollRows(delta: number): void;
  scrollToBottom(): void;
  /** Opens the pane's keyboard: reading a transcript must not cost typing. */
  focus(): void;
}

/** Rows paged in when the reader reaches the top of the current screen. */
const SCROLLBACK_PAGE_ROWS = 5;

export class TranscriptView {
  readonly element: HTMLElement;

  private readonly lines: HTMLElement;
  private readonly jump: HTMLButtonElement;
  private readonly source: TranscriptSource;
  private readonly keys: string[] = [];
  private following = true;
  private disposed = false;

  constructor(document: Document, source: TranscriptSource) {
    this.source = source;
    this.element = document.createElement("div");
    this.element.className = "transcript";
    // A transcript is a log, not a live region that re-reads itself: screen
    // readers get the role without being interrupted on every streamed frame.
    this.element.setAttribute("role", "log");
    this.element.tabIndex = 0;
    this.lines = document.createElement("div");
    this.lines.className = "transcript-lines";
    this.jump = document.createElement("button");
    this.jump.type = "button";
    this.jump.className = "transcript-jump";
    this.jump.textContent = "Jump to latest";
    this.jump.hidden = true;
    this.jump.onclick = () => this.jumpToLatest();
    this.element.append(this.lines, this.jump);
    this.element.addEventListener("scroll", this.onScroll, { passive: true });
    // Reading view or not, a tap on the pane is still how the operator opens
    // the keyboard for it.
    this.element.addEventListener("click", this.onClick);
  }

  /** Re-reads the grid and repaints the lines whose content actually changed. */
  update(): void {
    if (this.disposed) return;
    const lines = transcriptLines(this.source.rows(), this.source.theme());
    this.renderLines(lines);
    if (this.following) this.pinTail();
  }

  private renderLines(lines: readonly TranscriptLine[]): void {
    const document = this.element.ownerDocument;
    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index]!;
      const key = transcriptLineKey(line);
      const existing = this.lines.children[index] as HTMLElement | undefined;
      if (existing && this.keys[index] === key) continue;
      const node = existing ?? document.createElement("p");
      node.className = line.blank ? "transcript-line blank" : "transcript-line";
      // The hanging indent is the whole point of the reflow: a wrapped
      // continuation stays inside its own gutter instead of restarting at
      // column 0 the way the squeezed grid did.
      node.style.paddingLeft = line.indent > 0 ? `${line.indent}ch` : "";
      node.style.textIndent = line.indent > 0 ? `-${line.indent}ch` : "";
      node.replaceChildren(...line.segments.map((segment) => {
        const span = document.createElement("span");
        span.textContent = segment.text;
        span.style.color = `rgb(${segment.foreground.r}, ${segment.foreground.g}, ${segment.foreground.b})`;
        if (segment.background) {
          span.style.backgroundColor = `rgb(${segment.background.r}, ${segment.background.g}, ${segment.background.b})`;
        }
        if (segment.bold) span.style.fontWeight = "600";
        if (segment.italic) span.style.fontStyle = "italic";
        const decoration = [segment.underline ? "underline" : "", segment.strikethrough ? "line-through" : ""]
          .filter(Boolean)
          .join(" ");
        if (decoration) span.style.textDecoration = decoration;
        return span;
      }));
      if (!existing) this.lines.append(node);
      this.keys[index] = key;
    }
    while (this.lines.children.length > lines.length) {
      this.lines.lastElementChild?.remove();
      this.keys.pop();
    }
  }

  private pinTail(): void {
    this.element.scrollTop = this.element.scrollHeight;
    this.setJumpVisible(false);
  }

  private readonly onScroll = (): void => {
    if (this.disposed) return;
    const viewport = {
      scrollTop: this.element.scrollTop,
      scrollHeight: this.element.scrollHeight,
      clientHeight: this.element.clientHeight,
    };
    this.following = shouldFollowTail(viewport);
    this.setJumpVisible(!this.following);
    // Past the top of the current screen there is nothing more to scroll to in
    // the DOM: history lives in the emulator's scrollback, so the gesture pages
    // that back instead of dead-ending.
    if (shouldPageScrollback({ scrollTop: viewport.scrollTop, hasScrollbackAbove: this.source.hasScrollbackAbove() })) {
      this.source.scrollRows(-SCROLLBACK_PAGE_ROWS);
    }
  };

  private readonly onClick = (event: MouseEvent): void => {
    if (this.disposed || event.target === this.jump) return;
    if (this.element.ownerDocument.getSelection()?.isCollapsed === false) return;
    this.source.focus();
  };

  private setJumpVisible(visible: boolean): void {
    if (this.jump.hidden !== !visible) this.jump.hidden = !visible;
  }

  jumpToLatest(): void {
    this.following = true;
    this.source.scrollToBottom();
    this.update();
    this.pinTail();
  }

  /** True while new output pins the tail rather than leaving the reader in place. */
  get isFollowing(): boolean {
    return this.following;
  }

  dispose(): void {
    this.disposed = true;
    this.element.removeEventListener("scroll", this.onScroll);
    this.element.removeEventListener("click", this.onClick);
    this.element.remove();
  }
}
