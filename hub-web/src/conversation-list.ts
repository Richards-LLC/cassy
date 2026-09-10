import { escapeHtml, projectBadge } from "./cloud-brand";

export interface ConversationRow {
  key: string;
  machineId: string;
  session: string;
  supervisor: string;
  projectDir?: string;
  host: string;
  freshness: string;
  connection: string;
  attention: number;
  selected: boolean;
}

/** Keyed buttons: a catalog heartbeat must never steal keyboard focus. */
export class ConversationList {
  private nodes = new Map<string, HTMLButtonElement>();
  render(container: HTMLElement, rows: readonly ConversationRow[], open: (row: ConversationRow) => void): void {
    const current = new Set(rows.map((row) => row.key));
    for (const [key, node] of this.nodes) {
      if (!current.has(key) || node.parentElement !== container) { node.remove(); this.nodes.delete(key); }
    }
    rows.forEach((row, index) => {
      let node = this.nodes.get(row.key);
      if (!node) {
        node = container.ownerDocument.createElement("button");
        node.type = "button";
        node.className = "conversation-row";
        this.nodes.set(row.key, node);
      }
      node.dataset.threadKey = row.key;
      node.setAttribute("aria-current", String(row.selected));
      node.onclick = () => open(row);
      const markup = `${projectBadge(row.projectDir)}<strong class="conversation-supervisor">${escapeHtml(row.supervisor)}</strong><span class="conversation-host">${escapeHtml(row.host)} · ${escapeHtml(row.connection)}</span><span class="conversation-row-state">${row.attention ? `<span class="conversation-attention">${row.attention} need you</span>` : ""}</span><span class="conversation-freshness">${escapeHtml(row.freshness)}</span>`;
      if (node.innerHTML !== markup) node.innerHTML = markup;
      if (container.children[index] !== node) container.insertBefore(node, container.children[index] ?? null);
    });
  }
}
