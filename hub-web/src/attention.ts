import type { AttentionItem } from "./types";

export type AttentionSeverity = "critical" | "warning" | "info";
export type AttentionAction = "repair" | "view_pane" | "retry" | "open_pr" | "none";

export interface AttentionContent {
  headline: string;
  detail?: string;
  cause?: string;
  severity: AttentionSeverity;
  action: AttentionAction;
  ticketId?: string;
  payload?: unknown;
  fingerprint?: string;
}

export interface AttentionCard {
  key: string;
  content: AttentionContent;
  items: AttentionItem[];
  latest: AttentionItem;
  count: number;
}

export interface AttentionGroup {
  key: string;
  machineId?: string;
  machineLabel: string;
  session?: string;
  cards: AttentionCard[];
  count: number;
  worstSeverity: AttentionSeverity;
  overflow?: boolean;
}

export type AttentionCounts = Record<AttentionSeverity, number>;

const SEVERITY_RANK: Record<AttentionSeverity, number> = { critical: 0, warning: 1, info: 2 };
const LEGACY_DIAGNOSTIC_PREFIX = "Daemon ended: ";
const TASK_PREFIX = /^(cas-[0-9a-f]{4,16}):\s+(.+)$/i;
const URL = /https:\/\/[^\s<>"']+/i;
const MAX_SUMMARY_LENGTH = 90;

const CRITICAL_KINDS = new Set([
  "daemon_disconnected", "daemon_died", "daemon_ended", "auth_blocked", "auth_loss",
  "repair_required", "ci_failed", "ci_hard_failure", "build_failed", "build_hard_failure",
  "session_transport", "session_unreachable", "pane_exited",
]);
const WARNING_KINDS = new Set([
  "awaiting_merge", "retry", "retry_loop", "retrying", "hub_disconnected", "reconnecting",
  "connection_degraded", "degraded_connection", "config_drift",
]);

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

function normalizedKind(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/[\s-]+/g, "_")
    .toLowerCase();
}

function sentenceCase(value: string): string {
  return value.replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function clampSummary(value: string): string {
  const clean = value.replace(/\s+/g, " ").trim();
  return clean.length <= MAX_SUMMARY_LENGTH ? clean : `${clean.slice(0, MAX_SUMMARY_LENGTH - 1).trimEnd()}…`;
}

function looksLikePayload(value: string): boolean {
  const trimmed = value.trim();
  return trimmed.startsWith("{") || trimmed.startsWith("[") || /"[\w-]+"\s*:/.test(trimmed);
}

function severityFromStored(value: AttentionItem["severity"]): AttentionSeverity | undefined {
  if (value === "critical" || value === "warning" || value === "info") return value;
  if (value === "incident") return "critical";
  if (value === "notice") return "info";
  return undefined;
}

export function severityForEvent(kind: string, proposed?: AttentionItem["severity"]): AttentionSeverity {
  const normalized = normalizedKind(kind);
  const deterministic = CRITICAL_KINDS.has(normalized)
    ? "critical"
    : WARNING_KINDS.has(normalized)
      ? "warning"
      : "info";
  const candidate = severityFromStored(proposed);
  return candidate && SEVERITY_RANK[candidate] < SEVERITY_RANK[deterministic] ? candidate : deterministic;
}

export function actionForEvent(kind: string): AttentionAction {
  const normalized = normalizedKind(kind);
  if (["auth_blocked", "auth_loss", "repair_required"].includes(normalized)) return "repair";
  if (["awaiting_merge"].includes(normalized)) return "open_pr";
  if (["retry", "retry_loop", "retrying", "hub_disconnected", "reconnecting", "ci_failed", "ci_hard_failure", "build_failed", "build_hard_failure", "daemon_disconnected", "daemon_died", "daemon_ended"].includes(normalized)) return "retry";
  if (["session_transport", "session_unreachable", "pane_exited"].includes(normalized)) return "view_pane";
  return "none";
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
    detail: typeof nextAction === "string" && nextAction.trim()
      ? nextAction
      : "Inspect the factory daemon log and session metadata before reconnecting.",
    cause: daemonCause(envelope?.cause),
    severity: "critical",
    action: "retry",
    payload: diagnostic,
  };
}

export function machineEventAttention(kind: string, diagnostic: unknown): AttentionContent {
  if (normalizedKind(kind) === "daemon_disconnected") return daemonAttention(diagnostic);
  if (normalizedKind(kind) === "pane_exited") {
    return {
      headline: "Worker stopped",
      detail: "Open the session to inspect the worker and its terminal output.",
      severity: "critical",
      action: "view_pane",
      payload: diagnostic,
    };
  }
  if (normalizedKind(kind) === "session_removed") {
    return {
      headline: "Session ended",
      detail: "Open the machine to confirm whether the session was intentionally removed.",
      severity: "info",
      action: "none",
      payload: diagnostic,
    };
  }
  return {
    headline: sentenceCase(kind),
    severity: severityForEvent(kind),
    action: actionForEvent(kind),
    payload: diagnostic,
  };
}

function storedHeadline(item: AttentionItem): string {
  const stored = item.headline ?? item.message;
  if (!item.session) return stored;

  const ownContext = `${item.machineLabel} / ${item.session}:`;
  if (!stored.startsWith(ownContext)) return stored;
  const remainder = stored.slice(ownContext.length);
  return /^\s/.test(remainder) ? remainder.trimStart() : stored;
}

export function attentionContent(item: AttentionItem): AttentionContent {
  // Re-derive on every render. A stale record or older client cannot put a
  // baked machine/session prefix, leading ticket id, or JSON blob into the UI.
  const stored = storedHeadline(item);
  if (stored.startsWith(LEGACY_DIAGNOSTIC_PREFIX)) {
    try {
      return daemonAttention(JSON.parse(stored.slice(LEGACY_DIAGNOSTIC_PREFIX.length)));
    } catch {
      return daemonAttention(undefined);
    }
  }

  const severity = severityForEvent(item.kind, item.severity);
  const task = stored.match(TASK_PREFIX);
  const fallback = sentenceCase(normalizedKind(item.kind));
  const headline = task?.[2]
    ?? (looksLikePayload(stored) ? fallback : item.headline ? stored : sentenceCase(stored));

  return {
    headline: clampSummary(headline || fallback),
    detail: item.detail,
    cause: item.cause,
    severity,
    action: item.action ?? actionForEvent(item.kind),
    ticketId: item.ticketId ?? task?.[1].toLowerCase(),
    payload: item.payload,
    fingerprint: item.fingerprint,
  };
}

export function createAttentionItem(
  base: Pick<AttentionItem, "id" | "machineId" | "machineLabel" | "session" | "kind" | "createdAt">,
  content: string | AttentionContent,
): AttentionItem {
  const normalized: AttentionContent = typeof content === "string"
    ? { headline: content, severity: severityForEvent(base.kind), action: actionForEvent(base.kind) }
    : { ...content, severity: severityForEvent(base.kind, content.severity), action: content.action ?? actionForEvent(base.kind) };
  return {
    ...base,
    message: normalized.headline,
    headline: normalized.headline,
    detail: normalized.detail,
    cause: normalized.cause,
    severity: normalized.severity,
    action: normalized.action,
    ticketId: normalized.ticketId,
    payload: normalized.payload,
    fingerprint: normalized.fingerprint,
  };
}

function cardFingerprint(item: AttentionItem, content: AttentionContent): string {
  return item.fingerprint
    ?? content.fingerprint
    ?? [item.machineId, item.session ?? "machine", normalizedKind(item.kind), content.headline, content.detail ?? "", content.cause ?? ""].join("\u001f").toLowerCase();
}

export function coalesceAttention(items: readonly AttentionItem[]): AttentionCard[] {
  const cards = new Map<string, AttentionCard>();
  for (const item of items.filter((candidate) => !candidate.acknowledgedAt)) {
    const content = attentionContent(item);
    const key = cardFingerprint(item, content);
    const existing = cards.get(key);
    if (!existing) {
      cards.set(key, { key, content, items: [item], latest: item, count: 1 });
      continue;
    }
    existing.items.push(item);
    existing.count += 1;
    if (item.createdAt > existing.latest.createdAt) {
      existing.latest = item;
      existing.content = content;
    }
  }
  return [...cards.values()].sort((left, right) => {
    const severity = SEVERITY_RANK[left.content.severity] - SEVERITY_RANK[right.content.severity];
    return severity || right.latest.createdAt.localeCompare(left.latest.createdAt);
  });
}

function appendCard(group: AttentionGroup, card: AttentionCard): void {
  group.cards.push(card);
  group.count += card.count;
  if (SEVERITY_RANK[card.content.severity] < SEVERITY_RANK[group.worstSeverity]) {
    group.worstSeverity = card.content.severity;
  }
}

export function groupAttention(items: readonly AttentionItem[], maxGroups = 6): AttentionGroup[] {
  const groups: AttentionGroup[] = [];
  for (const card of coalesceAttention(items)) {
    const item = card.latest;
    const key = `${item.machineId}:${item.session ?? "machine"}`;
    const previous = groups.at(-1);
    if (previous?.key === key) {
      appendCard(previous, card);
      continue;
    }
    groups.push({
      key,
      machineId: item.machineId,
      machineLabel: item.machineLabel,
      session: item.session,
      cards: [card],
      count: card.count,
      worstSeverity: card.content.severity,
    });
  }

  if (groups.length <= maxGroups) return groups;
  const visible = groups.slice(0, Math.max(0, maxGroups - 1));
  const hidden = groups.slice(Math.max(0, maxGroups - 1));
  const overflow: AttentionGroup = {
    key: "attention-overflow",
    machineLabel: `Earlier activity · ${hidden.length} groups`,
    cards: [],
    count: 0,
    worstSeverity: "info",
    overflow: true,
  };
  for (const group of hidden) for (const card of group.cards) appendCard(overflow, card);
  return [...visible, overflow];
}

export function attentionCounts(items: readonly AttentionItem[]): AttentionCounts {
  const counts: AttentionCounts = { critical: 0, warning: 0, info: 0 };
  for (const item of items) {
    if (!item.acknowledgedAt) counts[attentionContent(item).severity] += 1;
  }
  return counts;
}

export function relativeTime(createdAt: string, now = Date.now()): string {
  const elapsed = Math.max(0, now - Date.parse(createdAt));
  if (elapsed < 60_000) return "now";
  if (elapsed < 3_600_000) return `${Math.floor(elapsed / 60_000)}m`;
  if (elapsed < 86_400_000) return `${Math.floor(elapsed / 3_600_000)}h`;
  return `${Math.floor(elapsed / 86_400_000)}d`;
}

export function attentionPayload(item: AttentionItem): string {
  const payload = item.payload ?? item.message;
  if (typeof payload === "string") return payload;
  try {
    return JSON.stringify(payload, null, 2);
  } catch {
    return String(payload);
  }
}

export function attentionUrl(item: AttentionItem): string | undefined {
  return attentionPayload(item).match(URL)?.[0];
}
