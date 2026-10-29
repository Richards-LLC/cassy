/**
 * Pebble thread model (cas-d167).
 *
 * Turns the flat `ConversationHistory` event list into what the thread paints:
 * day separators, runs of turns from one side grouped so their adjacent
 * corners tighten, consecutive short supervisor status turns folded into one
 * quiet line, and one timestamp per group. A long status, or one that asks
 * the operator for something, is not folded: it reads as a supervisor bubble. Pure, so grouping survives incremental
 * DOM updates — the view re-derives the classes from this model instead of
 * leaning on `:first-of-type`, which forgets a group once a status line or a
 * later turn lands inside it.
 */
import type { ConversationEvent, ConversationSend } from "./conversation-history";
import type { OperatorReply, OperatorTurnKind } from "./types";
import type { WorkEvent } from "./work-events";

export type ThreadSide = "you" | "supervisor";

export interface ThreadTurn {
  /** Stable identity: `send:<client id>` or `reply:<notification id>`. */
  key: string;
  side: ThreadSide;
  kind: OperatorTurnKind | "send";
  event: ConversationEvent;
  /** First turn in its group carries the group's open outer corner. */
  first: boolean;
  last: boolean;
}

export interface ThreadGroup {
  type: "group";
  key: string;
  side: ThreadSide;
  turns: ThreadTurn[];
  /** Shown once beneath the group, never once per bubble. */
  time: string | undefined;
  /** The turn whose time is shown was stamped by a machine clock ahead of this browser, so the time is its arrival (cas-1f13). */
  clockAhead?: boolean;
}

export interface ThreadCoalesce {
  type: "coalesce";
  key: string;
  /** Number of status turns folded into this line. */
  count: number;
  /** Text of the most recent status. */
  latest: string;
  replies: OperatorReply[];
  time: string | undefined;
  clockAhead?: boolean;
  /** The session the folded statuses came from (cas-8d52): it names the speaker. */
  session?: string;
}

export interface ThreadDay { type: "day"; key: string; label: string }
export interface ThreadSession { type: "session"; key: string; label: string }
export interface ThreadHistoryEnd { type: "history-end"; key: string; label: string }
export interface ThreadWorking { type: "working"; key: string }
/**
 * One stretch of the supervisor's work between two conversation turns
 * (cas-ff22): its thinking, tool calls and task events, oldest first.
 * Keyed by its first event, so events appended to it repaint this one node.
 */
export interface ThreadWork { type: "work"; key: string; events: WorkEvent[] }

export type ThreadItem = ThreadDay | ThreadSession | ThreadGroup | ThreadCoalesce | ThreadHistoryEnd | ThreadWorking | ThreadWork;

export interface ThreadModelOptions {
  /** Bounds a rendered group so a single speaker cannot defeat windowing. */
  groupSize?: number;
  /** Whether the supervisor is executing: appends the working line. */
  working?: boolean;
  /** Clock used for day labels; injectable for tests. */
  now?: number;
  /** The oldest loaded page is complete; paint its explicit end marker. */
  historyEnd?: boolean;
  /**
   * The session the thread belongs to (cas-55a4). Its own turns need no
   * "session … started" divider: the thread is that session.
   */
  session?: string;
  /**
   * Show work (cas-ff22): the session's work events, oldest first. Each run
   * between two turns becomes one `work` item. Events older than the oldest
   * loaded turn wait for that turn's page unless history is complete.
   * Absent, the model is exactly the conversation's.
   */
  work?: readonly WorkEvent[];
}

export function eventKey(event: ConversationEvent): string {
  return event.kind === "send" ? `send:${event.value.id}` : `reply:${event.value.notification_id}`;
}

/** The time a turn shows: its own time, not the sort key a live event was clamped to (cas-ac1f). */
function eventAt(event: ConversationEvent): number | undefined { return event.shownAt ?? event.at; }

export function clockLabel(at: number | undefined): string | undefined {
  if (at === undefined) return undefined;
  const date = new Date(at);
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

/**
 * The time each turn shows, in thread order (cas-1f13). The thread is in the
 * machine's order, and a time never reads later than a turn below it, than
 * the moment this browser received it, or than now: each turn shows the
 * earliest of its own time (capped at its arrival), every later turn's and
 * now. The operator's own message is the exception: it shows the time it was
 * sent. A stamp from a clock that runs ahead therefore shows the time the
 * turns after it arrived, never a future day. `clockAhead` is set when that
 * pulled the minute back, or the thread marked a live turn from a machine it
 * has seen running ahead. A few seconds of skew changes nothing on screen, so
 * it earns no hint.
 */
export function shownTimes(events: readonly ConversationEvent[], now: number): Array<{ at: number | undefined; clockAhead: boolean }> {
  const shown: Array<{ at: number | undefined; clockAhead: boolean }> = new Array(events.length);
  let floor = now;
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index]!;
    const stamp = eventAt(event);
    if (stamp === undefined) { shown[index] = { at: undefined, clockAhead: false }; continue; }
    const own = Math.min(stamp, event.arrivedAt ?? stamp);
    floor = Math.min(own, floor);
    // The operator's own message keeps the time it was sent: it is this
    // browser's clock, never the machine's, so it is never pulled back or
    // marked. It still bounds the turns above it.
    if (event.kind === "send") { shown[index] = { at: own, clockAhead: false }; continue; }
    // cas-9e33: a turn this browser saw arrive live is shown at that arrival,
    // a time it observed; only a later turn pulling it back marks it, beside
    // the mark the visit gave it. Any other turn is marked when its machine
    // stamp reads a different minute from the time shown.
    const measured = event.seenLive ? own : event.at ?? stamp;
    shown[index] = { at: floor, clockAhead: event.clockAhead === true || Math.floor(measured / 60_000) !== Math.floor(floor / 60_000) };
  }
  return shown;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/**
 * A turn's time as it is shown (cas-e829): "17:20" today, "Sep 30, 17:20" on
 * any other day, with the year when it is not this year. A bare clock time
 * from yesterday read as today's.
 */
export function stampLabel(at: number | undefined, now: number): string | undefined {
  const clock = clockLabel(at);
  if (at === undefined || clock === undefined) return undefined;
  if (dayKey(at) === dayKey(now)) return clock;
  const date = new Date(at);
  const year = date.getFullYear() === new Date(now).getFullYear() ? "" : ` ${date.getFullYear()}`;
  return `${MONTHS[date.getMonth()]} ${date.getDate()}${year}, ${clock}`;
}

function dayKey(at: number): string {
  const date = new Date(at);
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

// Date#toLocaleDateString constructs a formatter on every call. History
// presentation reuses these labels across frames and should reuse its formatters.
const sameYearDay = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", month: "short" });
const otherYearDay = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric" });

export function dayLabel(at: number, now: number): string {
  const key = dayKey(at);
  if (key === dayKey(now)) return "Today";
  if (key === dayKey(now - 86_400_000)) return "Yesterday";
  const date = new Date(at);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  return (sameYear ? sameYearDay : otherYearDay).format(date);
}

function turnOf(event: ConversationEvent): ThreadTurn {
  if (event.kind === "send") return { key: eventKey(event), side: "you", kind: "send", event, first: false, last: false };
  return { key: eventKey(event), side: "supervisor", kind: event.value.kind ?? "answer", event, first: false, last: false };
}

/**
 * A status longer than this reads as a message, not a progress tick: folding it
 * into the quiet line would clamp most of it away (P2 of the 2026-09-22 design
 * review; the real case is a 1,079-character "WAITING ON YOU" update).
 */
export const STATUS_FOLD_LIMIT = 140;

/**
 * Whether a status puts something in front of the operator: a waiting-on-you
 * marker, a request for their decision or input, or a direct question. Such a
 * status must never be set in the quietest style in the thread.
 */
const STATUS_ASK = /\bwaiting on (?:you|your)\b|\b(?:need|needs|needed) (?:you|your)\b|\byour (?:call|decision|answer|approval|input|go-ahead|sign-off)\b|\bover to you\b|\?(?:\s|$)/i;

export function statusAsks(message: string): boolean {
  return STATUS_ASK.test(message);
}

/** Fenced source must reach Markdown rendering, regardless of status length. */
const STATUS_FENCE = /^ {0,3}(?:`{3,}|~{3,})/m;

/** Only a short, plain status with no ask folds into the quiet coalesced line. */
export function foldsAsStatus(message: string): boolean {
  return message.trim().length <= STATUS_FOLD_LIMIT && !statusAsks(message) && !STATUS_FENCE.test(message);
}

/**
 * Turns this far apart start a new group with their own time (cas-d043 H14):
 * an answer at 12:00 and another at 12:08 read as two moments, not one.
 */
export const GROUP_SPLIT_MS = 5 * 60_000;

/** Derive the painted thread from the history's events. */
export function threadModel(events: readonly ConversationEvent[], options: ThreadModelOptions = {}): ThreadItem[] {
  const now = options.now ?? Date.now();
  const items: ThreadItem[] = [];
  let lastDay: string | undefined;
  let lastSession: string | undefined;
  let group: ThreadGroup | undefined;
  let coalesce: ThreadCoalesce | undefined;

  /** The shown time of the open group's latest dated turn (cas-d043 H14). */
  let groupLastAt: number | undefined;
  const closeGroup = (): void => {
    groupLastAt = undefined;
    if (!group) return;
    const first = group.turns[0], last = group.turns[group.turns.length - 1];
    if (first) first.first = true;
    if (last) last.last = true;
    group = undefined;
  };

  const work = options.work ?? [];
  let nextWork = 0;
  /** Paint the work done before `until` (all of it when undefined) as one item. */
  const flushWork = (until: number | undefined): void => {
    const start = nextWork;
    while (nextWork < work.length && (until === undefined || work[nextWork]!.at < until)) nextWork += 1;
    if (nextWork === start) return;
    closeGroup(); coalesce = undefined;
    const run = work.slice(start, nextWork);
    // Work that opens the thread sits under its own day, not above the first label.
    if (lastDay === undefined) {
      lastDay = dayKey(run[0]!.at);
      items.push({ type: "day", key: `day:${lastDay}`, label: dayLabel(run[0]!.at, now) });
    }
    items.push({ type: "work", key: `work:${run[0]!.id}`, events: run });
  };

  const times = shownTimes(events, now);
  events.forEach((event, index) => {
    // Queue order survives live catch-up; its arrival sort key can be newer
    // than the durable turns after it. Place work by the displayed clock too.
    const { at, clockAhead } = times[index]!;
    if (at !== undefined) {
      // Work older than the loaded history waits for its page.
      if (index === 0 && !options.historyEnd) while (nextWork < work.length && work[nextWork]!.at < at) nextWork += 1;
      else flushWork(at);
    }
    if (at !== undefined) {
      const day = dayKey(at);
      if (day !== lastDay) {
        closeGroup(); coalesce = undefined;
        lastDay = day;
        // A clock-skewed turn can bring a day back (tomorrow's stamp, then
        // today again): keep every separator's key unique (cas-ac1f).
        const dayKeyed = items.some((item) => item.key === `day:${day}`) ? `day:${day}:${items.length}` : `day:${day}`;
        items.push({ type: "day", key: dayKeyed, label: dayLabel(at, now) });
      }
    } else if (lastDay === undefined && items.length === 0) {
      // Undated history still reads as a conversation, not a bare list.
      lastDay = dayKey(now);
      items.push({ type: "day", key: `day:${lastDay}`, label: "Today" });
    }

    // cas-8d52 (journey F13): a turn with no session recorded (an older
    // daemon's history row) belongs to the session the thread is attached to,
    // so it no longer opens a "session … started" line below itself.
    const session = event.session ?? options.session;
    // The thread's own session opening the thread needs no line (cas-55a4).
    if (session && index === 0 && session === options.session) lastSession = session;
    if (session && session !== lastSession) {
      closeGroup(); coalesce = undefined;
      lastSession = session;
      items.push({
        type: "session",
        key: `session:${session}:${eventKey(event)}`,
        label: `session ${session} started ${clockLabel(at) ?? "earlier"}`,
      });
    }

    if (event.kind === "reply" && (event.value.kind ?? "answer") === "status" && foldsAsStatus(event.value.message)) {
      closeGroup();
      if (!coalesce) {
        coalesce = { type: "coalesce", key: `coalesce:${event.value.notification_id}`, count: 0, latest: "", replies: [], time: undefined, ...(event.session ? { session: event.session } : {}) };
        items.push(coalesce);
      }
      coalesce.count += 1;
      coalesce.latest = event.value.message;
      coalesce.replies.push(event.value);
      coalesce.time = stampLabel(at, now) ?? coalesce.time;
      // The hint belongs to the time shown, which is the latest turn's.
      if (at !== undefined) coalesce.clockAhead = clockAhead;
      return;
    }
    coalesce = undefined;

    const turn = turnOf(event);
    // cas-d043 H14: one time shows beneath a group, so turns minutes apart
    // are not merged under the later time and its clock-ahead mark: they
    // start a new group. (Splitting on the mark alone would regroup a thread
    // on reload, where a first live turn's mark is settled differently.)
    const apart = group !== undefined && at !== undefined && groupLastAt !== undefined && at - groupLastAt >= GROUP_SPLIT_MS;
    if (!group || group.side !== turn.side || apart || group.turns.length >= (options.groupSize ?? Infinity)) {
      closeGroup();
      group = { type: "group", key: `group:${turn.key}`, side: turn.side, turns: [], time: undefined };
      items.push(group);
    }
    group.turns.push(turn);
    group.time = stampLabel(at, now) ?? group.time;
    if (at !== undefined) { group.clockAhead = clockAhead; groupLastAt = at; }
  });
  if (events.length > 0) flushWork(undefined);
  closeGroup();
  // An empty durable page owns the empty state. Do not let pagination or a
  // transient working signal turn it into a fake history marker/thread.
  if (events.length > 0) {
    if (options.historyEnd) items.unshift({ type: "history-end", key: "history-end", label: "No earlier history" });
    // cas-71f4 (journey F20): while the newest message still says "Sending…",
    // that bubble is the one sending signal; the working line follows once the
    // supervisor has it.
    const newest = events.at(-1);
    const sending = newest?.kind === "send" && newest.value.state === "sending" && !newest.value.held;
    if (options.working && !sending) items.push({ type: "working", key: "working" });
  }
  return items;
}

/** A single status reads as its own text; a run counts the rest. */
export function coalesceText(item: ThreadCoalesce): string {
  return item.count > 1 ? `${item.count - 1} more update${item.count === 2 ? "" : "s"} · ${item.latest}` : item.latest;
}

export function sendOf(turn: ThreadTurn): ConversationSend | undefined {
  return turn.event.kind === "send" ? turn.event.value : undefined;
}

export function replyOf(turn: ThreadTurn): OperatorReply | undefined {
  return turn.event.kind === "reply" ? turn.event.value : undefined;
}

/* ---- evidence tables inside a message ---------------------------------- */

export interface EvidenceTable {
  header: string[] | undefined;
  rows: string[][];
}

export type MessageBlock = { type: "text"; text: string } | { type: "table"; table: EvidenceTable; source: string };

/** Outside pipes must be real delimiters, never escaped cell content. */
function splitCells(line: string): string[] | undefined {
  const source = line.trim();
  if (!source.startsWith("|")) return undefined;
  const cells: string[] = [];
  let cell = "";
  let lastDelimiter = false;
  for (let index = 1; index < source.length; index += 1) {
    const char = source[index]!;
    if (char === "\\" && source[index + 1] === "\\") {
      cell += "\\\\"; index += 1; lastDelimiter = false;
    } else if (char === "\\" && source[index + 1] === "|") {
      cell += "|"; index += 1; lastDelimiter = false;
    } else if (char === "|") {
      cells.push(cell.trim()); cell = ""; lastDelimiter = true;
    } else { cell += char; lastDelimiter = false; }
  }
  return lastDelimiter ? cells : undefined;
}

/** CSV consumes displayed cell text, keeping embedded newlines and Unicode. */
export function tableCsv(rows: string[][]): string {
  return rows.map(row => row.map(cell => /[",\r\n]/.test(cell) ? `"${cell.replace(/"/g, '""')}"` : cell).join(",")).join("\r\n");
}

/**
 * Split a supervisor message into prose and markdown-ish tables. A table is a
 * run of `| a | b |` lines; a `|---|---|` rule after the first row marks it as
 * the header. Prose keeps its own line breaks.
 */
export function messageBlocks(message: string): MessageBlock[] {
  const blocks: MessageBlock[] = [];
  // Keep source bytes for fenced-code Copy, including original separators.
  const parts = message.split(/(\r\n|\r|\n)/);
  const lines = parts.filter((_, index) => index % 2 === 0);
  let text: string[] = [];
  let fence: string | undefined;
  const appendTextLine = (index: number): void => { text.push(parts.slice(index * 2, index * 2 + 2).join("")); };
  const flushText = (): void => {
    const joined = fence ? text.join("").trimStart() : text.join("").trim();
    if (joined) blocks.push({ type: "text", text: joined });
    text = [];
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]!;
    const marker = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      appendTextLine(index);
      if (marker && marker[1]![0] === fence[0] && marker[1]!.length >= fence.length && !marker[2]!.trim()) fence = undefined;
      continue;
    }
    if (marker && !(marker[1]![0] === "`" && marker[2]!.includes("`"))) {
      fence = marker[1]!;
      appendTextLine(index);
      continue;
    }
    if (!splitCells(line)) { appendTextLine(index); continue; }
    const rows: string[][] = [];
    let header: string[] | undefined;
    const start = index;
    while (index < lines.length) {
      const cells = splitCells(lines[index]!);
      if (!cells) break;
      if (cells.every(cell => /^:?-{2,}:?$/.test(cell))) { if (rows.length === 1 && header === undefined) header = rows.pop(); }
      else rows.push(cells);
      index += 1;
    }
    const source = parts.slice(start * 2, index * 2 - 1).join("");
    index -= 1;
    if (rows.length === 0 && header === undefined) continue;
    flushText();
    blocks.push({ type: "table", table: { header, rows }, source });
  }
  flushText();
  return blocks;
}

/** Cell tone: pass/ok reads green, flake/warn amber, fail/error crit. */
export function cellTone(cell: string): "pass" | "flake" | "fail" | undefined {
  const value = cell.trim().toLowerCase();
  if (/^(pass|passed|ok|green|✓)$/.test(value)) return "pass";
  if (/\b(flake|flaky|warn|warning|skipped)\b/.test(value)) return "flake";
  if (/\b(fail|failed|error|red)\b/.test(value)) return "fail";
  return undefined;
}

/* ---- blocker evidence ---------------------------------------------------- */

const EVIDENCE_LINE = /^`?(?:[\w.@~-]+\/)*[\w.-]+\.\w+:\d+(?::\d+)?(?:\s*[·:—-]\s*.+)?`?$|^`[^`]+`$/;

/**
 * A blocker's evidence is its last line when that line reads as a file:line
 * reference (optionally `· label`) or is wrapped in backticks. It leaves the
 * prose and goes into the object's inset window.
 */
export function blockerEvidence(message: string): { text: string; evidence: string | undefined } {
  const lines = message.trimEnd().split(/\r?\n/);
  const last = lines.at(-1)?.trim() ?? "";
  if (lines.length < 2 || !EVIDENCE_LINE.test(last)) return { text: message, evidence: undefined };
  return { text: lines.slice(0, -1).join("\n").trimEnd(), evidence: last.replace(/^`|`$/g, "") };
}
