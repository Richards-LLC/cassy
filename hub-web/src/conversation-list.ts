import { escapeHtml, projectTitle } from "./cloud-brand";
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
  /** The same time in words for assistive tech ("20 minutes ago"), when it differs from `when` (cas-6acf). */
  whenSpoken?: string;
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
  /** When the session last did anything (ms epoch), for grouping and its time (cas-55a4). */
  activityAt?: number;
  /** When the session started (ms epoch): ranks Most recent when no session of a group has activity (cas-6acf). */
  startedAt?: number;
  /**
   * One of several live sessions of a project on one machine (cas-55a4). The
   * rows sit together under one heading; the most recently active one is
   * marked, so the others never read as copies of it.
   */
  group?: { key: string; label: string; size: number; first: boolean; active: boolean };
  /** This device may end the session (factory-manage), offered on grouped and dormant rows. */
  canEnd?: boolean;
}

/**
 * Put the rows of one project's live sessions on one machine together, most
 * recently active first, at the place of the group's first row; mark the most
 * recent one (cas-55a4). Rows of a project with one session are unchanged.
 */
export function groupConversationRows<T extends ConversationRow>(rows: readonly T[]): T[] {
  const groupKey = (row: T): string => `${row.machineId}\u0000${row.projectDir ?? `session:${row.session}`}`;
  const members = new Map<string, T[]>();
  for (const row of rows) {
    const key = groupKey(row);
    members.set(key, [...(members.get(key) ?? []), row]);
  }
  const out: T[] = [];
  const placed = new Set<string>();
  for (const row of rows) {
    const key = groupKey(row);
    if (placed.has(key)) continue;
    placed.add(key);
    const group = members.get(key)!;
    if (group.length < 2) { out.push({ ...row, group: undefined }); continue; }
    // cas-6acf: the newest activity leads; with none in the group, the newest start does.
    const anyActivity = group.some((member) => member.activityAt !== undefined);
    const rank = (member: T): number => (anyActivity ? member.activityAt : member.startedAt) ?? -Infinity;
    const ordered = [...group].sort((a, b) => rank(b) - rank(a));
    const label = `${projectTitle(row.projectDir) ?? row.supervisor} · ${group.length} sessions on ${machineName(row.host)}`;
    ordered.forEach((member, index) => out.push({ ...member, group: { key, label, size: group.length, first: index === 0, active: index === 0 && rank(member) !== -Infinity } }));
  }
  return out;
}

/**
 * A row's time from the session's own activity (cas-6acf): "now" under a
 * minute, then minutes, hours and days. Never a catalog-check time, and no
 * ticking seconds that make the conversation just used look older than an
 * idle one.
 */
export function activityTime(at: number, now: number = Date.now()): { short: string; spoken: string } {
  const elapsed = Math.max(0, now - at);
  const unit = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"} ago`;
  if (elapsed < 60_000) return { short: "now", spoken: "just now" };
  if (elapsed < 3_600_000) { const minutes = Math.floor(elapsed / 60_000); return { short: `${minutes}m`, spoken: unit(minutes, "minute") }; }
  if (elapsed < 86_400_000) { const hours = Math.floor(elapsed / 3_600_000); return { short: `${hours}h`, spoken: unit(hours, "hour") }; }
  const days = Math.floor(elapsed / 86_400_000);
  return { short: `${days}d`, spoken: unit(days, "day") };
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
  return `${projectTitle(row.projectDir) ?? ""} ${row.host} ${row.supervisor}`.toLocaleLowerCase();
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
  // cas-6acf: assistive tech hears the time in words ("20 minutes ago"), not "20m".
  const time = row.when
    ? `<span class="conversation-when${waiting ? " hot" : ""}" title="${escapeHtml(row.freshness)}"${row.whenSpoken ? ' aria-hidden="true"' : ""}>${escapeHtml(row.when)}</span>`
    : "";
  const spoken = row.when && row.whenSpoken ? `<span class="sr-only">, ${escapeHtml(row.whenSpoken)}</span>` : "";
  const count = unread > 0 ? `<span class="conversation-unread" aria-label="${unread} unread">${unread}</span>` : "";
  const flag = waiting ? `<span class="conversation-flag" role="img" aria-label="${row.attention === 1 ? "Waiting for you" : `${row.attention} waiting for you`}"></span>` : "";
  const marks = count || flag ? `<span class="conversation-marks">${count}${flag}</span>` : "";
  // No project named: the codename is the title (cas-1ca1 F03), not a status phrase.
  const project = projectTitle(row.projectDir);
  // The title is "project · machine": the project leads, never a dot (P13);
  // the one separator rides with the machine, so a dot never dangles at a
  // line end when a long project pushes the machine to the next line. The
  // machine is legible as text on every row (operator direction): the
  // monogram and accent alone do not name it. A long machine name ellipsises
  // inside the title column (its title attribute carries it whole) instead of
  // running under the time stamp (cas-1ca1). The codename sits beneath.
  // cas-55a4: among one project's sessions, the most recently active one says so.
  const mark = row.group?.active ? `<span class="conversation-session-mark">Most recent</span>` : "";
  return `<span class="conversation-avatar" aria-hidden="true">${escapeHtml(machineMonogram(row.host))}</span>`
    + `<span class="conversation-who"><span class="conversation-title"><strong class="conversation-project${project ? "" : " codename"}">${escapeHtml(project ?? row.supervisor)}</strong><span class="conversation-machine" title="${escapeHtml(machineName(row.host))}"><span class="conversation-sep" aria-hidden="true"></span><span class="conversation-machine-name">${escapeHtml(machineName(row.host))}</span></span></span>${project ? `<span class="conversation-supervisor codename">${escapeHtml(row.supervisor)}${mark}</span>` : mark}</span>`
    + time
    + `<span class="conversation-preview${row.unreachable ? " unreachable" : row.interrupted ? " interrupted" : waiting || unread > 0 ? " bold" : ""}">${escapeHtml(preview)}</span>`
    + marks
    + spoken;
}

/**
 * After scrollIntoView, a sub-pixel remainder could leave the last row's
 * buttons a fraction past the list's edge (cas-339a: 0.44px on a desktop).
 * Scroll the list by the whole remainder.
 */
function revealWhole(control: HTMLElement): void {
  const list = control.parentElement;
  if (!list) return;
  const edge = list.getBoundingClientRect().bottom;
  const lowest = Math.max(...[...control.querySelectorAll("button")].map((button) => button.getBoundingClientRect().bottom));
  if (Number.isFinite(lowest) && lowest > edge) list.scrollTop += Math.ceil(lowest - edge) + 1;
}

/** A power glyph: End session's face on a phone, where the words would crowd the row (cas-d6bf). */
const END_ICON = `<svg class="conversation-end-icon" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true" focusable="false"><path d="M10 3v6"/><path d="M6.2 5.6a6 6 0 1 0 7.6 0"/></svg>`;

/** Keyed buttons: a catalog heartbeat must never steal keyboard focus. */
export class ConversationList {
  private nodes = new Map<string, HTMLButtonElement>();
  /** Group headings and End session controls, keyed beside the rows (cas-55a4). */
  private extras = new Map<string, HTMLElement>();
  /** Rows whose End session is asking for confirmation, or ending. */
  private ending = new Map<string, "confirm" | "ending" | { error: string }>();
  /**
   * The newest render's arguments (cas-d6bf). An End session control is kept
   * across renders, so its own repaint must use these, never the container and
   * rows of the render that first built it: the shell rebuilds the list when a
   * conversation opens, and repainting into that detached list emptied the
   * live one until the next catalog poll.
   */
  private latest?: { container: HTMLElement; rows: readonly ConversationRow[]; open: (row: ConversationRow, event?: MouseEvent) => void; end?: (row: ConversationRow) => Promise<void> };
  render(container: HTMLElement, rows: readonly ConversationRow[], open: (row: ConversationRow, event?: MouseEvent) => void, end?: (row: ConversationRow) => Promise<void>): void {
    this.latest = { container, rows, open, end };
    const document = container.ownerDocument;
    const ordered: HTMLElement[] = [];
    const current = new Set(rows.map((row) => row.key));
    for (const [key, node] of this.nodes) {
      if (!current.has(key) || node.parentElement !== container) { node.remove(); this.nodes.delete(key); }
    }
    for (const key of this.ending.keys()) if (!current.has(key)) this.ending.delete(key);
    const extrasKept = new Set<string>();
    const extra = (key: string, make: () => HTMLElement): HTMLElement => {
      let node = this.extras.get(key);
      if (!node) { node = make(); this.extras.set(key, node); }
      extrasKept.add(key);
      return node;
    };
    for (const row of rows) {
      if (row.group?.first) {
        const head = extra(`group:${row.group.key}`, () => { const node = document.createElement("p"); node.className = "conversation-group-head"; return node; });
        if (head.textContent !== row.group.label) head.textContent = row.group.label;
        ordered.push(head);
      }
      let node = this.nodes.get(row.key);
      if (!node) {
        node = document.createElement("button");
        node.type = "button";
        this.nodes.set(row.key, node);
      }
      // The accent class rides on the row itself so both of a machine's projects share its colour.
      // cas-339a: a row End session sits beside keeps a column of its own for
      // it on a phone, so the row's time and marks are never under the button.
      const className = `conversation-row ${machineAccentClass(row.machineId)}${row.group ? " grouped" : ""}${row.canEnd && end ? " endable" : ""}`;
      if (node.className !== className) node.className = className;
      node.dataset.threadKey = row.key;
      node.dataset.machineId = row.machineId;
      node.dataset.waiting = String(row.attention > 0);
      node.dataset.unread = String(row.unread ?? 0);
      if (row.group) node.dataset.mostRecent = String(row.group.active); else delete node.dataset.mostRecent;
      node.setAttribute("aria-current", String(row.selected));
      node.onclick = (event) => open(row, event);
      const markup = conversationRowMarkup(row);
      if (node.innerHTML !== markup) node.innerHTML = markup;
      ordered.push(node);
      if (row.canEnd && end) {
        const control = extra(`end:${row.key}`, () => { const node = document.createElement("div"); node.className = "conversation-end"; return node; });
        this.renderEnd(control, row, end);
        ordered.push(control);
      }
    }
    for (const [key, node] of this.extras) {
      if (!extrasKept.has(key)) { node.remove(); this.extras.delete(key); }
    }
    ordered.forEach((node, index) => {
      if (container.children[index] !== node) container.insertBefore(node, container.children[index] ?? null);
    });
  }

  /**
   * Where focus goes once a session is ended (cas-e634): the row after it,
   * else the row before it, else its group's heading, else the list. Keys
   * and the group heading are taken before the row leaves the list.
   */
  private neighbours(key: string): { after?: string; before?: string; group?: string } {
    const rows = this.latest?.rows ?? [];
    const index = rows.findIndex((candidate) => candidate.key === key);
    const group = rows[index]?.group?.key;
    return { after: rows[index + 1]?.key, before: index > 0 ? rows[index - 1]?.key : undefined, group: group === undefined ? undefined : `group:${group}` };
  }

  /** The ended row is gone; if focus went with it, land it on the nearest thing left. */
  private landAfterEnd(control: HTMLElement, neighbours: { after?: string; before?: string; group?: string }): void {
    const document = control.ownerDocument;
    const active = document.activeElement;
    const lost = !active || active === document.body || !active.isConnected || control.contains(active);
    if (!lost) return;
    const row = (key?: string) => (key === undefined ? undefined : this.nodes.get(key));
    const live = (node?: HTMLElement) => (node?.isConnected ? node : undefined);
    const heading = live(neighbours.group === undefined ? undefined : this.extras.get(neighbours.group));
    const target = live(row(neighbours.after)) ?? live(row(neighbours.before)) ?? heading ?? live(this.latest?.container);
    if (!target) return;
    if (!(target instanceof HTMLButtonElement) && !target.hasAttribute("tabindex")) target.tabIndex = -1;
    target.focus({ preventScroll: false });
  }

  /**
   * End session, then a confirmation that names what stops (cas-55a4). Only
   * the confirmation's own button ends anything.
   */
  private renderEnd(control: HTMLElement, row: ConversationRow, end: (row: ConversationRow) => Promise<void>): void {
    const state = this.ending.get(row.key);
    const signature = JSON.stringify([row.supervisor, row.host, state]);
    if (control.dataset.signature === signature) return;
    control.dataset.signature = signature;
    control.dataset.state = state === undefined ? "idle" : typeof state === "object" ? "error" : state;
    const document = control.ownerDocument;
    const rerender = (): void => {
      const latest = this.latest;
      if (!latest) return;
      // A list rebuilt since the newest render is found again by its id.
      const container = latest.container.isConnected || !latest.container.id
        ? latest.container
        : latest.container.ownerDocument.getElementById(latest.container.id) ?? latest.container;
      this.render(container, latest.rows, latest.open, latest.end);
    };
    const button = (text: string, className: string, onclick: () => void): HTMLButtonElement => {
      const node = document.createElement("button"); node.type = "button"; node.className = className; node.textContent = text; node.onclick = onclick; return node;
    };
    if (state === undefined || typeof state === "object") {
      const ask = button("", "conversation-end-ask", () => {
        this.ending.set(row.key, "confirm");
        rerender();
        // The confirmation opens under the row: keep its buttons in view, focus on Cancel.
        control.scrollIntoView?.({ block: "nearest" });
        revealWhole(control);
        control.querySelector<HTMLButtonElement>(".conversation-end-cancel")?.focus({ preventScroll: true });
      });
      ask.innerHTML = `${END_ICON}<span class="conversation-end-label">End session</span>`;
      ask.setAttribute("aria-label", `End session ${row.supervisor} on ${machineName(row.host)}`);
      const children: Node[] = [ask];
      if (typeof state === "object") { const error = document.createElement("span"); error.className = "conversation-end-error"; error.setAttribute("role", "alert"); error.textContent = state.error; children.push(error); }
      control.replaceChildren(...children);
      return;
    }
    const question = document.createElement("span"); question.className = "conversation-end-question";
    question.textContent = state === "ending"
      ? `Ending ${row.supervisor}…`
      : `End ${row.supervisor} on ${machineName(row.host)}? Its supervisor and workers stop.`;
    if (state === "ending") {
      question.setAttribute("role", "status");
      // Focus waits on the status line while the end is in flight: the
      // confirmation that held it is gone (cas-e634).
      question.tabIndex = -1;
      control.replaceChildren(question);
      return;
    }
    const confirm = button("End session", "conversation-end-confirm danger", () => {
      const keyboard = control.contains(control.ownerDocument.activeElement);
      const neighbours = this.neighbours(row.key);
      this.ending.set(row.key, "ending");
      rerender();
      if (keyboard) control.querySelector<HTMLElement>(".conversation-end-question")?.focus({ preventScroll: true });
      void end(row).then(() => {
        this.ending.delete(row.key);
        this.landAfterEnd(control, neighbours);
      }, (error: unknown) => {
        this.ending.set(row.key, { error: `Could not end ${row.supervisor}: ${error instanceof Error ? error.message : String(error)}` });
        rerender();
      });
    });
    const cancel = button("Cancel", "conversation-end-cancel", () => {
      this.ending.delete(row.key);
      rerender();
      control.querySelector<HTMLButtonElement>(".conversation-end-ask")?.focus({ preventScroll: true });
    });
    control.replaceChildren(question, confirm, cancel);
  }
}
