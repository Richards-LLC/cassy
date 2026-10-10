import { escapeHtml, projectTitle } from "./cloud-brand";
import { joinSpoken } from "./spoken-names";
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
  /** cas-97d58 F19: a half-written reply parked in this conversation. */
  draft?: string;
  /**
   * The session's newest activity in plain words, from the catalog
   * ("Messaged bright-robin-85"). A grouped row shows it until the thread has
   * a turn of its own, so sibling sessions differ before any visit (cas-5d2c).
   */
  activityLine?: string;
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
    // cas-5d2c: the same noun as the footer's "3 conversations".
    const label = `${projectTitle(row.projectDir) ?? row.supervisor} · ${group.length} conversations on ${machineName(row.host)}`;
    ordered.forEach((member, index) => out.push({ ...member, group: { key, label, size: group.length, first: index === 0, active: index === 0 && rank(member) !== -Infinity } }));
  }
  return out;
}

/**
 * The catalog's "from → to" activity label (hub/mod.rs activity_label) in
 * plain words (cas-5d2c): "supervisor → Commander" is "Wrote to you",
 * "supervisor → bright-robin-85" is "Messaged bright-robin-85". No arrows and
 * no role jargon reach a row.
 */
export function plainActivity(label: string | undefined): string | undefined {
  if (!label) return undefined;
  const [from, to] = label.split(" → ").map((part) => part.trim());
  if (!from || !to) return label;
  if (from === "supervisor" && to === "Commander") return "Wrote to you";
  if (from === "Commander") return "You wrote to it";
  if (from === "supervisor" && to === "supervisor") return "Typed at its terminal";
  if (from === "supervisor") return `Messaged ${to}`;
  if (from === "lifecycle-wake" || from.endsWith("-wake")) return "Woken up";
  if (to === "supervisor") return `Heard from ${from}`;
  return `${from} to ${to}`;
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

/**
 * A machine-stamped activity time in this browser's time (cas-24fe). The hub
 * stamps a session's activity with the machine's clock. Once the thread has
 * measured that clock's lead, the stamp less the lead (never after now);
 * before that, a stamp in this browser's future dates from the moment this
 * page first saw it, kept in `seen` across renders. A clock that runs ahead
 * therefore never keeps a row at "now".
 */
export function machineActivityAt(
  stamp: number,
  lead: number | undefined,
  seen: { stamp: number; seen: number } | undefined,
  now: number,
): { at: number; seen?: { stamp: number; seen: number } } {
  if (lead !== undefined) return { at: Math.min(stamp - lead, now) };
  // Once dated by when it was seen, it stays so: the row ages steadily even after now passes the stamp.
  if (seen?.stamp === stamp) return { at: seen.seen, seen };
  if (stamp <= now) return { at: stamp };
  return { at: now, seen: { stamp, seen: now } };
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

/**
 * The row's spoken name (cas-d8a5, journey F32), set as its aria-label so the
 * parts never run together ("calm-puma-34Most recent") or leave a stray
 * " , " before the time: project on machine, codename, Most recent, preview,
 * time in words, unread and waiting.
 */
export function conversationRowSpokenName(row: ConversationRow): string {
  const project = projectTitle(row.projectDir);
  const fallback = row.group ? row.activityLine || row.connection : row.connection;
  const preview = truncateConversationPreview(plainTextMarkdown(rowPreviewText(row, fallback)));
  const unread = row.unread ?? 0;
  return joinSpoken([
    `${project ?? row.supervisor} on ${machineName(row.host)}`,
    project ? row.supervisor : undefined,
    row.group?.active ? "most recent" : undefined,
    preview,
    row.when ? row.whenSpoken ?? row.when : undefined,
    unread > 0 ? `${unread} unread` : undefined,
    row.attention > 0 ? (row.attention === 1 ? "waiting for you" : `${row.attention} waiting for you`) : undefined,
  ]);
}

/** One Pebble row: `avatar · project machine · codename · preview · time`, with
 * two distinct affordances — waiting (ochre dot, hot time) and unread (accent
 * count pill). Users think in projects (journey F7): the project is the title,
 * the machine follows it, and the generated codename is tertiary text. */
/**
 * What the row's second line says. A connection problem leads, so the row
 * agrees with the header (cas-a447); then a parked draft in another
 * conversation (cas-97d58 F19), so the operator sees where they left a reply;
 * then the last turn.
 */
function rowPreviewText(row: ConversationRow, fallback: string): string {
  if (row.unreachable || row.interrupted) return row.connection;
  const draft = row.draft?.trim();
  if (draft && !row.selected) return `Draft: ${draft}`;
  return row.preview || fallback;
}

export function conversationRowMarkup(row: ConversationRow): string {
  const waiting = row.attention > 0;
  const unread = row.unread ?? 0;
  // cas-5d2c: a grouped row with no turn of its own yet shows what its
  // session last did, so siblings differ before any of them is opened.
  const fallback = row.group ? row.activityLine || row.connection : row.connection;
  const preview = truncateConversationPreview(plainTextMarkdown(rowPreviewText(row, fallback)));
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
    + `<span class="conversation-preview${row.unreachable ? " unreachable" : row.interrupted ? " interrupted" : row.draft?.trim() && !row.selected ? " draft" : waiting || unread > 0 ? " bold" : ""}">${escapeHtml(preview)}</span>`
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

/** How long "<codename> on <machine> ended." stays where its row was (cas-f60a). */
export const ENDED_NOTICE_MS = 8_000;

/** Keyed buttons: a catalog heartbeat must never steal keyboard focus. */
/** A press that travels further than this is a drag or a scroll, not a tap (cas-4646). */
export const ROW_TAP_SLOP_PX = 10;
/**
 * How long a lifted press waits for its click before it opens the pressed row
 * itself (cas-4646). A mouse's click comes in the same task as its release; a
 * touch's tap follows the lifted finger at once, and a click that arrives
 * late, after the row opened without it, is absorbed until `absorb` passes.
 */
export const ROW_TAP_CLICK_WAIT_MS = { mouse: 0, touch: 300, absorb: 600 } as const;

export class ConversationList {
  private nodes = new Map<string, HTMLButtonElement>();
  /** Group headings and End session controls, keyed beside the rows (cas-55a4). */
  private extras = new Map<string, HTMLElement>();
  /** Rows whose End session is asking for confirmation, or ending. */
  /**
   * A failed end carries its sentence, and whether it must be announced
   * (cas-9ae6): only when focus is not brought back to End session, which
   * reads the sentence as its description; otherwise it would be said twice.
   */
  private ending = new Map<string, "confirm" | "ending" | { error: string; announce: boolean }>();
  /**
   * The newest render's arguments (cas-d6bf). An End session control is kept
   * across renders, so its own repaint must use these, never the container and
   * rows of the render that first built it: the shell rebuilds the list when a
   * conversation opens, and repainting into that detached list emptied the
   * live one until the next catalog poll.
   */
  private latest?: { container: HTMLElement; rows: readonly ConversationRow[]; open: (row: ConversationRow, event?: MouseEvent) => void; end?: (row: ConversationRow) => Promise<void> };
  /**
   * cas-f60a: the session that just ended, said where its row was and through
   * a polite live region, until ENDED_NOTICE_MS passes or another one ends.
   */
  private ended?: { text: string; after?: string; before?: string; group?: string; timer: ReturnType<typeof setTimeout> };
  private endedNode?: HTMLParagraphElement;
  /** The live region beside the list. It stays in the page, so a new sentence is announced. */
  private announcer?: HTMLParagraphElement;
  /**
   * cas-4646: the row a pointer pressed, held by its key rather than by its
   * node. A live update between press and release can move the row (a group
   * re-sorts by activity) or replace every row (the shell rebuilds when a
   * session starts or ends). The click that follows then lands on another
   * row, on nothing, or on a detached node, and the tap did nothing. The
   * gesture remembers what was pressed, so the press opens it either way.
   */
  private press?: { pointerId: number; pointerType: string; key: string; x: number; y: number; released: boolean; opened?: boolean; timer?: ReturnType<typeof setTimeout> };
  /** The document whose pointer events this list follows. */
  private watched?: Document;
  render(container: HTMLElement, rows: readonly ConversationRow[], open: (row: ConversationRow, event?: MouseEvent) => void, end?: (row: ConversationRow) => Promise<void>): void {
    this.latest = { container, rows, open, end };
    const document = container.ownerDocument;
    this.watch(document);
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
      const spoken = conversationRowSpokenName(row);
      if (node.getAttribute("aria-label") !== spoken) node.setAttribute("aria-label", spoken);
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
    if (this.ended) {
      const notice = this.endedNode ??= Object.assign(document.createElement("p"), { className: "conversation-ended" });
      if (notice.textContent !== this.ended.text) notice.textContent = this.ended.text;
      ordered.splice(this.endedPlace(ordered, rows, this.ended), 0, notice);
    } else this.endedNode?.remove();
    if (end) {
      const announcer = this.announcer ??= Object.assign(document.createElement("p"), { className: "sr-only conversation-ended-status" });
      announcer.setAttribute("role", "status");
      if (container.nextElementSibling !== announcer && container.parentElement) container.after(announcer);
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

  /**
   * Where the ended notice goes (cas-f60a): where the row was. Before the row
   * that followed it when that is in the same group, else after the row before
   * it (and its End control), else at the top.
   */
  /** Follow pointer presses on rows for the life of the page (cas-4646). */
  private watch(document: Document): void {
    if (this.watched === document) return;
    this.watched = document;
    const capture = { capture: true } as const;
    document.addEventListener("pointerdown", (event) => this.pressed(event), capture);
    document.addEventListener("pointermove", (event) => {
      const press = this.press;
      if (press && !press.released && event.pointerId === press.pointerId && Math.hypot(event.clientX - press.x, event.clientY - press.y) > ROW_TAP_SLOP_PX) this.forget();
    }, capture);
    document.addEventListener("pointercancel", (event) => { if (event.pointerId === this.press?.pointerId) this.forget(); }, capture);
    document.addEventListener("pointerup", (event) => this.lifted(event), capture);
    // Capture, so a click the list moved onto another row, or onto anything
    // else, is turned back into the pressed row before it is handled there.
    document.addEventListener("click", (event) => this.landed(event), capture);
  }

  private pressed(event: PointerEvent): void {
    this.forget();
    if (!event.isPrimary || event.button !== 0) return;
    const node = event.target instanceof Element ? event.target.closest<HTMLElement>(".conversation-row") : null;
    const key = node?.dataset.threadKey;
    if (!node || key === undefined || this.nodes.get(key) !== node) return;
    this.press = { pointerId: event.pointerId, pointerType: event.pointerType, key, x: event.clientX, y: event.clientY, released: false };
  }

  private lifted(event: PointerEvent): void {
    const press = this.press;
    if (!press || press.released || event.pointerId !== press.pointerId) return;
    if (Math.hypot(event.clientX - press.x, event.clientY - press.y) > ROW_TAP_SLOP_PX) { this.forget(); return; }
    press.released = true;
    // A click that never comes (its target left the page) still opens the row.
    const touch = press.pointerType !== "mouse";
    press.timer = setTimeout(() => {
      if (this.press !== press) return;
      press.opened = true;
      const view = this.watched?.defaultView;
      const PointerClick = view?.PointerEvent ?? view?.MouseEvent;
      this.openKey(press.key, PointerClick ? new PointerClick("click", { detail: 1, pointerType: press.pointerType } as PointerEventInit) : undefined);
      // A mouse's click would have come by now; a late tap is absorbed for a while.
      if (!touch) { if (this.press === press) this.forget(); return; }
      press.timer = setTimeout(() => { if (this.press === press) this.forget(); }, ROW_TAP_CLICK_WAIT_MS.absorb);
    }, touch ? ROW_TAP_CLICK_WAIT_MS.touch : ROW_TAP_CLICK_WAIT_MS.mouse);
  }

  /** A click after a lifted press opens the pressed row, wherever it landed. */
  private landed(event: MouseEvent): void {
    const press = this.press;
    if (!press?.released || event.detail === 0) return;
    this.forget();
    // The row already opened without its click; this late click is that tap.
    if (press.opened) { event.preventDefault(); event.stopPropagation(); return; }
    const node = event.target instanceof Element ? event.target.closest<HTMLElement>(".conversation-row") : null;
    if (node && node.dataset.threadKey === press.key && this.nodes.get(press.key) === node) return;
    event.preventDefault();
    event.stopPropagation();
    this.openKey(press.key, event);
  }

  private openKey(key: string, event: MouseEvent | undefined): void {
    const row = this.latest?.rows.find((item) => item.key === key);
    if (row) this.latest!.open(row, event);
  }

  private forget(): void {
    if (this.press?.timer !== undefined) clearTimeout(this.press.timer);
    this.press = undefined;
  }

  private endedPlace(ordered: readonly HTMLElement[], rows: readonly ConversationRow[], ended: { after?: string; before?: string; group?: string }): number {
    const at = (node?: HTMLElement) => (node ? ordered.indexOf(node) : -1);
    const after = ended.after === undefined ? -1 : at(this.nodes.get(ended.after));
    if (after >= 0 && rows.find((row) => row.key === ended.after)?.group?.key === ended.group) return after;
    if (ended.before !== undefined) {
      const before = Math.max(at(this.nodes.get(ended.before)), at(this.extras.get(`end:${ended.before}`)));
      if (before >= 0) return before + 1;
    }
    return 0;
  }

  /** Say that a session ended, in place and to assistive tech (cas-f60a). */
  private announceEnded(row: ConversationRow, neighbours: { after?: string; before?: string }): void {
    if (this.ended) clearTimeout(this.ended.timer);
    const text = `${row.supervisor} on ${machineName(row.host)} ended.`;
    const timer = setTimeout(() => {
      if (this.ended?.timer !== timer) return;
      this.ended = undefined;
      if (this.announcer) this.announcer.textContent = "";
      this.repaint();
    }, ENDED_NOTICE_MS);
    this.ended = { text, after: neighbours.after, before: neighbours.before, group: row.group?.key, timer };
    this.repaint();
    if (this.announcer) this.announcer.textContent = text;
  }

  /** Render again with the newest arguments, into the live list. */
  private repaint(): void {
    const latest = this.latest;
    if (!latest) return;
    // A list rebuilt since the newest render is found again by its id.
    const container = latest.container.isConnected || !latest.container.id
      ? latest.container
      : latest.container.ownerDocument.getElementById(latest.container.id) ?? latest.container;
    this.render(container, latest.rows, latest.open, latest.end);
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
    const rerender = (): void => this.repaint();
    const button = (text: string, className: string, onclick: (event: MouseEvent) => void): HTMLButtonElement => {
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
      if (typeof state === "object") {
        const error = document.createElement("span");
        error.id = `conversation-end-error:${row.key}`;
        error.className = "conversation-end-error";
        // Said once (cas-9ae6): an alert when focus stays where the operator
        // went; as End session's description when focus is brought back to it.
        if (state.announce) error.setAttribute("role", "alert");
        error.textContent = state.error;
        ask.setAttribute("aria-describedby", error.id);
        children.push(error);
      }
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
    // cas-f60a: the confirmation opens where End session was, so the second
    // click of a double-click (detail 2 and up) lands in it. Neither button
    // takes that click: only a deliberate click, tap or key confirms or cancels.
    const confirm = button("End session", "conversation-end-confirm danger", (event) => {
      if (event.detail > 1) return;
      const keyboard = control.contains(control.ownerDocument.activeElement);
      const neighbours = this.neighbours(row.key);
      this.ending.set(row.key, "ending");
      rerender();
      if (keyboard) control.querySelector<HTMLElement>(".conversation-end-question")?.focus({ preventScroll: true });
      void end(row).then(() => {
        this.ending.delete(row.key);
        this.announceEnded(row, neighbours);
        this.landAfterEnd(control, neighbours);
      }, () => {
        // Keep the failed row actionable, but leave focus alone if the operator
        // moved elsewhere while the request was in flight (cas-a549).
        const heldFocus = control.contains(document.activeElement);
        this.ending.set(row.key, { error: `Could not end ${row.supervisor} on ${machineName(row.host)}. Try End session again. If it still fails, check the session on ${machineName(row.host)}.`, announce: !heldFocus });
        rerender();
        if (heldFocus) {
          control.scrollIntoView?.({ block: "nearest" });
          revealWhole(control);
          control.querySelector<HTMLButtonElement>(".conversation-end-ask")?.focus({ preventScroll: true });
        }
      });
    });
    const cancel = button("Cancel", "conversation-end-cancel", (event) => {
      if (event.detail > 1) return;
      this.ending.delete(row.key);
      rerender();
      control.querySelector<HTMLButtonElement>(".conversation-end-ask")?.focus({ preventScroll: true });
    });
    // Cancel first, where the pointer that opened this already is (cas-f60a).
    control.replaceChildren(question, cancel, confirm);
    // A double-click's second press on the question would take focus off
    // Cancel (and select a word): it does neither (cas-f60a).
    control.onmousedown = (event) => { if (event.detail > 1) event.preventDefault(); };
  }
}
