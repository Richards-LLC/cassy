import { escapeHtml, projectBadge } from "./cloud-brand";
import { machineAccentClass, machineMonogram } from "./machine-accent";

export interface ConversationRow {
  key: string;
  machineId: string;
  session: string;
  supervisor: string;
  projectDir?: string;
  /** Machine label; its first letter is the avatar monogram. */
  host: string;
  /** Long-form freshness, the title of the time cell. */
  freshness: string;
  connection: string;
  /** Short time at the right of the headline ("09:58", "Tue", "1m"). */
  when?: string;
  /** Last turn in the thread, or the connection state when there is none. */
  preview?: string;
  /** Asks and blockers waiting on the operator: ochre dot + hot timestamp. */
  attention: number;
  /** Supervisor turns the operator has not opened: filled count pill in the machine accent. */
  unread?: number;
  selected: boolean;
}

/** One Pebble row: `avatar · name · project · preview · time`, with two distinct
 * affordances — waiting (ochre dot, hot time) and unread (accent count pill). */
export function conversationRowMarkup(row: ConversationRow): string {
  const waiting = row.attention > 0;
  const unread = row.unread ?? 0;
  const preview = row.preview || row.connection;
  const time = row.when ? `<span class="conversation-when${waiting ? " hot" : ""}" title="${escapeHtml(row.freshness)}">${escapeHtml(row.when)}</span>` : "";
  const headlineEnd = unread > 0 ? `<span class="conversation-unread" aria-label="${unread} unread">${unread}</span>` : time;
  const flag = waiting ? `<span class="conversation-flag" role="img" aria-label="${row.attention === 1 ? "Waiting for you" : `${row.attention} waiting for you`}"></span>` : "";
  return `<span class="conversation-avatar" aria-hidden="true">${escapeHtml(machineMonogram(row.host))}</span>`
    + `<span class="conversation-who"><strong class="conversation-supervisor">${escapeHtml(row.supervisor)}</strong><span class="conversation-sep" aria-hidden="true"></span>${projectBadge(row.projectDir)}</span>`
    + headlineEnd
    + `<span class="conversation-preview${waiting || unread > 0 ? " bold" : ""}">${escapeHtml(preview)}</span>`
    + flag;
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
        this.nodes.set(row.key, node);
      }
      // The accent class rides on the row itself so both of a machine's projects share its colour.
      const className = `conversation-row ${machineAccentClass(row.machineId)}`;
      if (node.className !== className) node.className = className;
      node.dataset.threadKey = row.key;
      node.dataset.machineId = row.machineId;
      node.dataset.waiting = String(row.attention > 0);
      node.dataset.unread = String(row.unread ?? 0);
      node.setAttribute("aria-current", String(row.selected));
      node.onclick = () => open(row);
      const markup = conversationRowMarkup(row);
      if (node.innerHTML !== markup) node.innerHTML = markup;
      if (container.children[index] !== node) container.insertBefore(node, container.children[index] ?? null);
    });
  }
}
