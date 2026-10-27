/**
 * Commander work events (cas-ff22): the client side of the daemon's work-event
 * stream (`docs/guides/commander-work-events.md`). Pages are read by cursor,
 * details on demand, and a daemon that pushes `WorkEventsAppended` says when
 * to read the newest page again. Everything here is data: labels and details
 * are agent-supplied text, and the views set them only as `textContent`.
 */

export type WorkEventKind = "thinking" | "tool" | "todo" | "task" | "worker" | "notice";

export interface WorkEvent {
  readonly id: string;
  readonly kind: WorkEventKind;
  readonly session: string;
  readonly agent?: string;
  readonly turn?: string;
  /** Milliseconds since the epoch. */
  readonly at: number;
  readonly label: string;
  readonly hasDetail: boolean;
  readonly error: boolean;
}

export interface WorkEventsPage {
  readonly request_id: string;
  readonly events: WorkEvent[];
  readonly has_earlier: boolean;
  readonly next_before?: string;
}

export interface WorkEventDetail {
  readonly request_id: string;
  readonly event_id: string;
  readonly detail?: string;
  readonly truncated: boolean;
}

/** `pull`: pages and details only. `live`: change notices and per-agent timelines too. */
export type WorkEventSupport = "none" | "pull" | "live";

const KINDS: readonly WorkEventKind[] = ["thinking", "tool", "todo", "task", "worker", "notice"];

export function workEventSupport(capabilities: unknown): WorkEventSupport {
  if (!Array.isArray(capabilities) || !capabilities.includes("work_events")) return "none";
  return capabilities.includes("work_events_live") ? "live" : "pull";
}

function workEvent(value: unknown): WorkEvent | undefined {
  if (!value || typeof value !== "object") return undefined;
  const row = value as Record<string, unknown>;
  const at = typeof row.at === "string" ? Date.parse(row.at) : NaN;
  if (typeof row.id !== "string" || !row.id || !Number.isFinite(at) || typeof row.label !== "string") return undefined;
  // An unknown kind reads as a notice (schema v1).
  const kind = KINDS.includes(row.kind as WorkEventKind) ? row.kind as WorkEventKind : "notice";
  return {
    id: row.id,
    kind,
    session: typeof row.session === "string" ? row.session : "",
    ...(typeof row.agent === "string" && row.agent ? { agent: row.agent } : {}),
    ...(typeof row.turn === "string" && row.turn ? { turn: row.turn } : {}),
    at,
    label: row.label,
    hasDetail: row.has_detail === true,
    error: row.error === true,
  };
}

export function workEventsPage(value: unknown): WorkEventsPage | undefined {
  if (!value || typeof value !== "object") return undefined;
  const page = value as Record<string, unknown>;
  if (typeof page.request_id !== "string" || !Array.isArray(page.events)) return undefined;
  return {
    request_id: page.request_id,
    events: page.events.map(workEvent).filter((event): event is WorkEvent => event !== undefined),
    has_earlier: page.has_earlier === true,
    ...(typeof page.next_before === "string" ? { next_before: page.next_before } : {}),
  };
}

export function workEventDetail(value: unknown): WorkEventDetail | undefined {
  if (!value || typeof value !== "object") return undefined;
  const detail = value as Record<string, unknown>;
  if (typeof detail.request_id !== "string" || typeof detail.event_id !== "string") return undefined;
  return {
    request_id: detail.request_id,
    event_id: detail.event_id,
    ...(typeof detail.detail === "string" ? { detail: detail.detail } : {}),
    truncated: detail.truncated === true,
  };
}

export type DetailState =
  | { readonly state: "loading" }
  | { readonly state: "ready"; readonly text: string; readonly truncated: boolean }
  | { readonly state: "gone" }
  | { readonly state: "failed"; readonly reason: string };

/** How many newest pages one change notice may read back to close a gap. */
const GAP_PAGES = 3;

/**
 * One stream's loaded events: the session stream, or one agent's timeline.
 * It merges pages by event id, keeps the cursor of its oldest page, and
 * remembers which requests are in flight so a page is asked for once.
 */
export class WorkEventStream {
  private readonly byId = new Map<string, WorkEvent>();
  private sorted: WorkEvent[] | undefined;
  private readonly details = new Map<string, DetailState>();
  /** request id → what it asked for. */
  private readonly inFlight = new Map<string, "newest" | "older" | "gap" | { detail: string }>();
  private gapPages = 0;
  /** The cursor of the oldest page read; undefined until the first page. */
  private oldestCursor: string | undefined;
  private earlier = false;
  private loadedOnce = false;
  /** Bumped on every change a view paints. */
  version = 0;

  constructor(readonly agent?: string) {}

  get events(): readonly WorkEvent[] {
    this.sorted ??= [...this.byId.values()].sort((left, right) => left.at - right.at || (left.id < right.id ? -1 : left.id > right.id ? 1 : 0));
    return this.sorted;
  }

  get loaded(): boolean { return this.loadedOnce; }
  get hasEarlier(): boolean { return this.earlier; }
  get cursor(): string | undefined { return this.oldestCursor; }
  /** The oldest loaded event's time. */
  get oldestAt(): number | undefined { return this.events[0]?.at; }

  /** Whether a page (not a detail) is being read. */
  get loading(): boolean {
    for (const kind of this.inFlight.values()) if (typeof kind === "string") return true;
    return false;
  }

  owns(requestId: string): boolean { return this.inFlight.has(requestId); }

  /** Note a sent request; `undefined` (not sent) is ignored. */
  requested(requestId: string | undefined, kind: "newest" | "older" | "gap" | { detail: string }): void {
    if (requestId === undefined) return;
    this.inFlight.set(requestId, kind);
    if (typeof kind === "object") { this.details.set(kind.detail, { state: "loading" }); this.version += 1; }
  }

  /** Whether a request of this kind is already out. */
  pending(kind: "newest" | "older"): boolean {
    for (const value of this.inFlight.values()) if (value === kind || (kind === "newest" && value === "gap")) return true;
    return false;
  }

  /**
   * Merge an answered page. Returns the cursor to read next when a newest
   * page did not reach the events already loaded (a gap), else undefined.
   */
  accept(page: WorkEventsPage): string | undefined {
    const kind = this.inFlight.get(page.request_id);
    if (kind === undefined || typeof kind === "object") return undefined;
    this.inFlight.delete(page.request_id);
    const known = this.byId.size > 0;
    const newestKnown = this.events.at(-1)?.at;
    let overlapped = !known;
    for (const event of page.events) {
      if (this.byId.has(event.id)) overlapped = true;
      this.byId.set(event.id, event);
    }
    if (known && newestKnown !== undefined && page.events.some((event) => event.at <= newestKnown)) overlapped = true;
    this.sorted = undefined;
    this.version += 1;
    if (!this.loadedOnce || kind === "older") {
      this.loadedOnce = true;
      this.oldestCursor = page.next_before;
      this.earlier = page.has_earlier;
      return undefined;
    }
    // A newest (or gap) page that never met the loaded events leaves a hole
    // between them: read on back from it, a bounded number of pages.
    if (!overlapped && page.has_earlier && page.next_before !== undefined) {
      this.gapPages = kind === "gap" ? this.gapPages + 1 : 1;
      if (this.gapPages <= GAP_PAGES) return page.next_before;
      // Too far behind: start over from this page instead of leaving a hole.
      const fresh = new Set(page.events.map((event) => event.id));
      for (const id of [...this.byId.keys()]) if (!fresh.has(id)) this.byId.delete(id);
      this.sorted = undefined;
      this.oldestCursor = page.next_before;
      this.earlier = page.has_earlier;
    }
    this.gapPages = 0;
    return undefined;
  }

  acceptDetail(detail: WorkEventDetail): boolean {
    const kind = this.inFlight.get(detail.request_id);
    if (kind === undefined || typeof kind !== "object") return false;
    this.inFlight.delete(detail.request_id);
    this.details.set(kind.detail, detail.detail === undefined ? { state: "gone" } : { state: "ready", text: detail.detail, truncated: detail.truncated });
    this.version += 1;
    return true;
  }

  /** A request was refused; a detail says why, a page may be asked for again. */
  failed(requestId: string, reason: string): boolean {
    const kind = this.inFlight.get(requestId);
    if (kind === undefined) return false;
    this.inFlight.delete(requestId);
    if (typeof kind === "object") this.details.set(kind.detail, { state: "failed", reason });
    this.version += 1;
    return true;
  }

  detail(eventId: string): DetailState | undefined { return this.details.get(eventId); }

  /** Forget in-flight requests whose answers can no longer arrive (the socket closed). */
  abandon(): void {
    for (const [id, kind] of this.inFlight) {
      if (typeof kind === "object") this.details.delete(kind.detail);
      this.inFlight.delete(id);
    }
    this.version += 1;
  }
}

/* ---- presentation helpers (pure) ---------------------------------------- */

/** Task transitions that read as a chip, in the words the daemon labels them with. */
const CHIP_VERBS: ReadonlyArray<[RegExp, ChipTone, string]> = [
  [/\bmerged\b/i, "ok", "merged"],
  [/\bparked for merge\b/i, "idle", "parked"],
  [/\bblocked\b/i, "crit", "blocked"],
  [/\bclose rejected\b/i, "crit", "close rejected"],
  [/\bclosed\b/i, "ok", "closed"],
  [/\breopened\b/i, "warn", "reopened"],
  [/\bstarted\b/i, "idle", "started"],
];

export type ChipTone = "ok" | "warn" | "crit" | "idle";

export interface TaskChip { readonly id: string; readonly subject: string; readonly verb: string; readonly tone: ChipTone; readonly label: string }

/** A task event as a chip: its task id and transition. Others stay rows. */
export function taskChip(event: WorkEvent): TaskChip | undefined {
  if (event.kind !== "task") return undefined;
  const match = CHIP_VERBS.find(([pattern]) => pattern.test(event.label));
  if (!match) return undefined;
  const subject = /^(\S+)\s/.exec(event.label)?.[1] ?? "task";
  return { id: event.id, subject, verb: match[2], tone: match[1], label: event.label };
}

/** The one-line summary of a collapsed work block. */
export function workSummary(events: readonly WorkEvent[]): string {
  const count = (kind: WorkEventKind): number => events.filter((event) => event.kind === kind).length;
  const tools = count("tool"), thinking = count("thinking"), todos = count("todo");
  const failed = events.filter((event) => event.kind === "tool" && event.error).length;
  const others = events.length - tools - thinking - todos - events.filter((event) => taskChip(event) !== undefined).length;
  const parts: string[] = [];
  if (thinking > 0) parts.push("Thought");
  if (tools > 0) parts.push(`ran ${tools} tool${tools === 1 ? "" : "s"}${failed > 0 ? ` (${failed} failed)` : ""}`);
  if (todos > 0) parts.push(`updated todos`);
  if (others > 0) parts.push(`${others} event${others === 1 ? "" : "s"}`);
  if (parts.length === 0) return "Task updates";
  const text = parts.join(", ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** The words a row shows: thinking reads as "Thinking", the rest as labelled. */
export function rowText(event: WorkEvent): string {
  if (event.kind === "thinking") return event.label ? `Thinking: ${event.label}` : "Thinking…";
  return event.label || event.kind;
}

/** A worker's state in the words of its timeline header. */
export function workerState(status: string | undefined, currentTask: string | undefined): "working" | "waiting" | "parked" | "idle" {
  const value = (status ?? "").toLowerCase();
  if (/parked|awaiting|merge/.test(value)) return "parked";
  if (/block|wait|stall/.test(value)) return "waiting";
  if (currentTask || /active|busy|work|running/.test(value)) return "working";
  return "idle";
}
