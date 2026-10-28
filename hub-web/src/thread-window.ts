import type { ThreadItem } from "./thread-model";

/** Measured, variable-height rows. Only the reading window enters the DOM. */
export class ThreadWindow {
  private readonly heights = new Map<string, number>();
  start = 0;
  end = 0;
  private items: readonly ThreadItem[] = [];
  private offsets: number[] = [0];
  private requested?: number;

  measure(nodes: ReadonlyMap<string, HTMLElement>): void {
    for (const [key, node] of nodes) {
      if (!node.isConnected) continue;
      const height = node.getBoundingClientRect().height;
      const gap = Number.parseFloat(node.ownerDocument.defaultView!.getComputedStyle(node.parentElement!).rowGap) || 0;
      if (height > 0) this.heights.set(key, height + gap);
    }
  }

  select(items: readonly ThreadItem[], top: number, height: number, following: boolean, anchor?: string): { items: readonly ThreadItem[]; above: number; below: number } {
    this.items = items;
    this.offsets = [0];
    for (const item of items) {
      // Before layout, paragraph-sized replies need more room than a status
      // line. A fixed per-bubble guess mounted many screens of Markdown on
      // the first frame, only to remove them after measuring that giant layout.
      const estimate = item.type === "group" ? item.turns.reduce((height, turn) => {
        const text = turn.event.kind === "send" ? turn.event.value.text : turn.event.value.message;
        const lines = Math.max(text.split("\n").length, Math.ceil(text.length / 60));
        return height + 80 + lines * 24;
      }, 0) : item.type === "coalesce" ? 100 : 40;
      this.offsets.push(this.offsets.at(-1)! + (this.heights.get(item.key) ?? estimate));
    }
    const total = this.offsets.at(-1)!;
    // Two screenfuls of margin, with a hard ceiling of 80 bubbles (20 groups).
    const viewport = Math.max(height, 600);
    let start = following ? Math.max(0, items.length - 20) : Math.max(0, this.offsets.findIndex(offset => offset > Math.max(0, top - viewport)) - 1);
    let end = following ? items.length : Math.min(items.length, start + 20);
    if (items.length <= 20) { start = 0; end = items.length; }
    else if (this.requested !== undefined) {
      start = Math.max(0, Math.min(this.requested, items.length - 1));
      end = Math.min(items.length, start + 20);
    } else if (!following && anchor) {
      const index = items.findIndex(item => item.key === anchor || (item.type === "group" && item.turns.some(turn => turn.key === anchor)));
      if (index >= 0 && (index < start || index >= end)) { start = Math.max(0, index - 6); end = Math.min(items.length, start + 20); }
    }
    if (following && this.requested === undefined && items.length > 20) {
      // Large bubbles need fewer rows; short updates still fill the viewport generously.
      while (start < end - 1 && total - this.offsets[start + 1]! >= viewport * 3) start++;
    }
    this.requested = undefined;
    this.start = start; this.end = end;
    return { items: items.slice(start, end), above: this.offsets[start]!, below: total - this.offsets[end]! };
  }

  earlier(): void { this.requested = Math.max(0, this.start - 10); }
  later(): void { this.requested = Math.min(this.items.length - 1, this.start + 10); }
  show(key: string): void {
    const index = this.items.findIndex(item => item.key === key || (item.type === "group" && item.turns.some(turn => turn.key === key)));
    if (index >= 0) this.requested = Math.max(0, index - 6);
  }
  get hasEarlier(): boolean { return this.start > 0; }
  get hasLater(): boolean { return this.end < this.items.length; }
}

/** Reconcile in place: replacing a parent's children detaches focused links/media. */
export function reconcileChildren(parent: HTMLElement, children: readonly HTMLElement[]): void {
  for (let index = 0; index < children.length; index++) {
    const child = children[index]!;
    if (parent.children[index] !== child) parent.insertBefore(child, parent.children[index] ?? null);
  }
  while (parent.children.length > children.length) parent.lastElementChild!.remove();
}
