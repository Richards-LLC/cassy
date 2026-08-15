import type { AttentionItem } from "./types";

export type AttentionSeverity = "incident" | "notice";

export interface AttentionContent {
  headline: string;
  detail?: string;
  cause?: string;
  severity: AttentionSeverity;
  ticketId?: string;
}

export interface AttentionGroup {
  machineId: string;
  machineLabel: string;
  session?: string;
  items: AttentionItem[];
}

const INCIDENT_KINDS = new Set(["daemon_disconnected", "session_transport", "pane_exited"]);
const TASK_PREFIX = /^([a-z]+-[a-z0-9]+):\s+(.+)$/i;
const LEGACY_DIAGNOSTIC_PREFIX = "Daemon ended: ";

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function sentenceCase(value: string): string {
  return value.replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function daemonCause(value: unknown): string | undefined {
  const cause = asRecord(value);
  const kind = cause?.kind;
  if (typeof kind !== "string" || !kind) return undefined;
  if (kind === "signal") {
    const name = cause.name;
    return typeof name === "string" && name ? `Process ended with ${name}` : "Process ended from a signal";
  }
  if (kind === "exit_code" || kind === "clean_exit") {
    const code = cause.code;
    return typeof code === "number" ? `Process exited with code ${code}` : sentenceCase(kind);
  }
  return sentenceCase(kind);
}

export function daemonAttention(diagnostic: unknown): AttentionContent {
  const envelope = asRecord(diagnostic);
  const nextAction = envelope?.next_action;
  return {
    headline: "Daemon connection lost",
    detail: typeof nextAction === "string" && nextAction.trim() ? nextAction : "Inspect the factory daemon log and session metadata before reconnecting.",
    cause: daemonCause(envelope?.cause),
    severity: "incident",
  };
}

export function machineEventAttention(kind: string, diagnostic: unknown): AttentionContent {
  if (kind === "daemon_disconnected") return daemonAttention(diagnostic);
  if (kind === "pane_exited") {
    return { headline: "Worker stopped", detail: "Open the session to inspect the worker and its terminal output.", severity: "incident" };
  }
  if (kind === "session_removed") {
    return { headline: "Session ended", detail: "Open the machine to confirm whether the session was intentionally removed.", severity: "notice" };
  }
  return { headline: sentenceCase(kind), severity: INCIDENT_KINDS.has(kind) ? "incident" : "notice" };
}

export function attentionContent(item: AttentionItem): AttentionContent {
  if (item.headline) {
    return {
      headline: item.headline,
      detail: item.detail,
      cause: item.cause,
      severity: item.severity ?? (INCIDENT_KINDS.has(item.kind) ? "incident" : "notice"),
      ticketId: item.ticketId,
    };
  }

  if (item.message.startsWith(LEGACY_DIAGNOSTIC_PREFIX)) {
    try {
      return daemonAttention(JSON.parse(item.message.slice(LEGACY_DIAGNOSTIC_PREFIX.length)));
    } catch {
      return daemonAttention(undefined);
    }
  }

  const task = item.message.match(TASK_PREFIX);
  if (task) {
    return { headline: task[2], severity: "notice", ticketId: task[1] };
  }
  return { headline: sentenceCase(item.message), severity: INCIDENT_KINDS.has(item.kind) ? "incident" : "notice" };
}

export function groupAttention(items: AttentionItem[]): AttentionGroup[] {
  const groups = new Map<string, AttentionGroup>();
  for (const item of items) {
    const key = `${item.machineId}:${item.session ?? "machine"}`;
    const group = groups.get(key) ?? {
      machineId: item.machineId,
      machineLabel: item.machineLabel,
      session: item.session,
      items: [],
    };
    group.items.push(item);
    groups.set(key, group);
  }
  return [...groups.values()];
}
