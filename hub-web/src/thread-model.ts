/**
 * Pebble thread model (cas-d167).
 *
 * Turns the flat `ConversationHistory` event list into what the thread paints:
 * day separators, runs of turns from one side grouped so their adjacent
 * corners tighten, consecutive supervisor status turns folded into one quiet
 * line, and one timestamp per group. Pure, so grouping survives incremental
 * DOM updates — the view re-derives the classes from this model instead of
 * leaning on `:first-of-type`, which forgets a group once a status line or a
 * later turn lands inside it.
 */
import type { ConversationEvent, ConversationSend } from "./conversation-history";
import type { OperatorReply, OperatorTurnKind } from "./types";

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
}

export interface ThreadDay { type: "day"; key: string; label: string }
export interface ThreadSession { type: "session"; key: string; label: string }
export interface ThreadHistoryEnd { type: "history-end"; key: string; label: string }
export interface ThreadWorking { type: "working"; key: string }

export type ThreadItem = ThreadDay | ThreadSession | ThreadGroup | ThreadCoalesce | ThreadHistoryEnd | ThreadWorking;

export interface ThreadModelOptions {
  /** Whether the supervisor is executing: appends the working line. */
  working?: boolean;
  /** Clock used for day labels; injectable for tests. */
  now?: number;
  /** The oldest loaded page is complete; paint its explicit end marker. */
  historyEnd?: boolean;
}

export function eventKey(event: ConversationEvent): string {
  return event.kind === "send" ? `send:${event.value.id}` : `reply:${event.value.notification_id}`;
}

function eventAt(event: ConversationEvent): number | undefined { return event.at; }

export function clockLabel(at: number | undefined): string | undefined {
  if (at === undefined) return undefined;
  const date = new Date(at);
  return `${String(date.getHours()).padStart(2, "0")}:${String(date.getMinutes()).padStart(2, "0")}`;
}

function dayKey(at: number): string {
  const date = new Date(at);
  return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
}

export function dayLabel(at: number, now: number): string {
  const key = dayKey(at);
  if (key === dayKey(now)) return "Today";
  if (key === dayKey(now - 86_400_000)) return "Yesterday";
  const date = new Date(at);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  return date.toLocaleDateString(undefined, sameYear ? { weekday: "short", day: "numeric", month: "short" } : { day: "numeric", month: "short", year: "numeric" });
}

function turnOf(event: ConversationEvent): ThreadTurn {
  if (event.kind === "send") return { key: eventKey(event), side: "you", kind: "send", event, first: false, last: false };
  return { key: eventKey(event), side: "supervisor", kind: event.value.kind ?? "answer", event, first: false, last: false };
}

/** Derive the painted thread from the history's events. */
export function threadModel(events: readonly ConversationEvent[], options: ThreadModelOptions = {}): ThreadItem[] {
  const now = options.now ?? Date.now();
  const items: ThreadItem[] = [];
  let lastDay: string | undefined;
  let lastSession: string | undefined;
  let group: ThreadGroup | undefined;
  let coalesce: ThreadCoalesce | undefined;

  const closeGroup = (): void => {
    if (!group) return;
    const first = group.turns[0], last = group.turns[group.turns.length - 1];
    if (first) first.first = true;
    if (last) last.last = true;
    group = undefined;
  };

  for (const event of events) {
    const at = eventAt(event);
    if (at !== undefined) {
      const day = dayKey(at);
      if (day !== lastDay) {
        closeGroup(); coalesce = undefined;
        lastDay = day;
        items.push({ type: "day", key: `day:${day}`, label: dayLabel(at, now) });
      }
    } else if (lastDay === undefined && items.length === 0) {
      // Undated history still reads as a conversation, not a bare list.
      lastDay = dayKey(now);
      items.push({ type: "day", key: `day:${lastDay}`, label: "Today" });
    }

    const session = event.session;
    if (session && session !== lastSession) {
      closeGroup(); coalesce = undefined;
      lastSession = session;
      items.push({
        type: "session",
        key: `session:${session}:${eventKey(event)}`,
        label: `session ${session} started ${clockLabel(at) ?? "earlier"}`,
      });
    }

    if (event.kind === "reply" && (event.value.kind ?? "answer") === "status") {
      closeGroup();
      if (!coalesce) {
        coalesce = { type: "coalesce", key: `coalesce:${event.value.notification_id}`, count: 0, latest: "", replies: [], time: undefined };
        items.push(coalesce);
      }
      coalesce.count += 1;
      coalesce.latest = event.value.message;
      coalesce.replies.push(event.value);
      coalesce.time = clockLabel(at) ?? coalesce.time;
      continue;
    }
    coalesce = undefined;

    const turn = turnOf(event);
    if (!group || group.side !== turn.side) {
      closeGroup();
      group = { type: "group", key: `group:${turn.key}`, side: turn.side, turns: [], time: undefined };
      items.push(group);
    }
    group.turns.push(turn);
    group.time = clockLabel(at) ?? group.time;
  }
  closeGroup();
  if (options.historyEnd) items.unshift({ type: "history-end", key: "history-end", label: "No earlier history" });
  if (options.working) items.push({ type: "working", key: "working" });
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

export type MessageBlock = { type: "text"; text: string } | { type: "table"; table: EvidenceTable };

const TABLE_ROW = /^\s*\|.*\|\s*$/;
const TABLE_RULE = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

function splitCells(line: string): string[] {
  return line.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map((cell) => cell.trim());
}

/**
 * Split a supervisor message into prose and markdown-ish tables. A table is a
 * run of `| a | b |` lines; a `|---|---|` rule after the first row marks it as
 * the header. Prose keeps its own line breaks.
 */
export function messageBlocks(message: string): MessageBlock[] {
  const blocks: MessageBlock[] = [];
  const lines = message.split(/\r?\n/);
  let text: string[] = [];
  const flushText = (): void => {
    const joined = text.join("\n").trim();
    if (joined) blocks.push({ type: "text", text: joined });
    text = [];
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]!;
    if (!TABLE_ROW.test(line)) { text.push(line); continue; }
    const rows: string[][] = [];
    let header: string[] | undefined;
    while (index < lines.length && TABLE_ROW.test(lines[index]!)) {
      const current = lines[index]!;
      if (TABLE_RULE.test(current)) { if (rows.length === 1 && header === undefined) header = rows.pop(); }
      else rows.push(splitCells(current));
      index += 1;
    }
    index -= 1;
    if (rows.length === 0 && header === undefined) continue;
    flushText();
    blocks.push({ type: "table", table: { header, rows } });
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
