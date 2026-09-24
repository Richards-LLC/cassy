import { escapeHtml, projectName } from "./cloud-brand";
import { machineAccentClass, machineMonogram } from "./machine-accent";
import { plainTextMarkdown } from "./markdown-renderer";

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
  /** The session left the catalog with an instruction still pending: the
   * preview line names that state instead of the last turn (cas-7294). */
  unreachable?: boolean;
  /** The conversation's connection is down (reconnecting, unreachable, needs
   * pairing): the preview line names that state instead of the last turn, so
   * the row agrees with the header and the footer (cas-a447). */
  interrupted?: boolean;
  /** Asks and blockers waiting on the operator: ochre dot + hot timestamp. */
  attention: number;
  /** Supervisor turns the operator has not opened: filled count pill in the machine accent. */
  unread?: number;
  selected: boolean;
}

export const CONVERSATION_PREVIEW_MAX_CHARS = 160;

/** Keep a long reply useful to screen readers and the two-line rail preview. */
export function truncateConversationPreview(text: string): string {
  const characters = Array.from(text);
  if (characters.length <= CONVERSATION_PREVIEW_MAX_CHARS) return text;
  return `${characters.slice(0, CONVERSATION_PREVIEW_MAX_CHARS - 1).join("").trimEnd()}…`;
}

/** The machine's own name: a host label such as "Atlas · Linux" reads "Atlas" on the row. */
export function machineName(host: string): string {
  const name = host.split(" · ")[0]?.trim();
  return name || host.trim();
}

/** The words a list search matches: project, machine and supervisor codename. */
export function conversationSearchText(row: Pick<ConversationRow, "projectDir" | "host" | "supervisor">): string {
  return `${projectName(row.projectDir)} ${row.host} ${row.supervisor}`.toLocaleLowerCase();
}

/** Rows whose project, machine or supervisor contains every word of the query (case-insensitive). */
export function filterConversationRows<T extends Pick<ConversationRow, "projectDir" | "host" | "supervisor">>(rows: readonly T[], query: string): T[] {
  const words = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...rows];
  return rows.filter((row) => { const text = conversationSearchText(row); return words.every((word) => text.includes(word)); });
}

/** One Pebble row: `avatar · project machine · codename · preview · time`, with
 * two distinct affordances — waiting (ochre dot, hot time) and unread (accent
 * count pill). Users think in projects (journey F7): the project is the title,
 * the machine follows it, and the generated codename is tertiary text. */
export function conversationRowMarkup(row: ConversationRow): string {
  const waiting = row.attention > 0;
  const unread = row.unread ?? 0;
  const preview = truncateConversationPreview(plainTextMarkdown(row.unreachable || row.interrupted ? row.connection : row.preview || row.connection));
  // The time always holds the headline end; an unread count sits beneath it
  // with the waiting dot, so the most active row never loses its time (P13).
  const time = row.when ? `<span class="conversation-when${waiting ? " hot" : ""}" title="${escapeHtml(row.freshness)}">${escapeHtml(row.when)}</span>` : "";
  const count = unread > 0 ? `<span class="conversation-unread" aria-label="${unread} unread">${unread}</span>` : "";
  const flag = waiting ? `<span class="conversation-flag" role="img" aria-label="${row.attention === 1 ? "Waiting for you" : `${row.attention} waiting for you`}"></span>` : "";
  const marks = count || flag ? `<span class="conversation-marks">${count}${flag}</span>` : "";
  // The title is "project · machine": the project leads, never a dot (P13);
  // the one separator rides with the machine, so a dot never dangles at a
  // line end when a long project pushes the machine to the next line. The
  // machine is legible as text on every row (operator direction): the
  // monogram and accent alone do not name it. The codename sits beneath.
  return `<span class="conversation-avatar" aria-hidden="true">${escapeHtml(machineMonogram(row.host))}</span>`
    + `<span class="conversation-who"><span class="conversation-title"><strong class="conversation-project">${escapeHtml(projectName(row.projectDir))}</strong><span class="conversation-machine"><span class="conversation-sep" aria-hidden="true"></span>${escapeHtml(machineName(row.host))}</span></span><span class="conversation-supervisor codename">${escapeHtml(row.supervisor)}</span></span>`
    + time
    + `<span class="conversation-preview${row.unreachable ? " unreachable" : row.interrupted ? " interrupted" : waiting || unread > 0 ? " bold" : ""}">${escapeHtml(preview)}</span>`
    + marks;
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
