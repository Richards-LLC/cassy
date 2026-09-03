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
  enrichmentPending?: boolean;
}

export interface AttentionEnrichment {
  severity: AttentionSeverity;
  summary: string;
  detail?: string | null;
  action: AttentionAction;
  fingerprint: string;
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
const MAX_DETAIL_LENGTH = 120;

interface DeterministicTemplate {
  headline: string;
  detail?: string;
  severity: AttentionSeverity;
  action: AttentionAction;
}

// This is the single deterministic type table. Keep severity, prose, and action
// together so a newly-known event cannot acquire a second, drifting mapper.
const MACHINE_EVENT_TEMPLATES: Record<string, DeterministicTemplate> = {
  daemon_disconnected: { headline: "Daemon connection lost", severity: "critical", action: "retry" },
  daemon_died: { headline: "Daemon stopped", severity: "critical", action: "retry" },
  daemon_ended: { headline: "Daemon stopped", severity: "critical", action: "retry" },
  auth_blocked: { headline: "Authentication blocked", severity: "critical", action: "repair" },
  auth_loss: { headline: "Authentication expired", severity: "critical", action: "repair" },
  repair_required: { headline: "Machine needs re-pairing", severity: "critical", action: "repair" },
  ci_failed: { headline: "CI failed", severity: "critical", action: "retry" },
  ci_hard_failure: { headline: "CI failed", severity: "critical", action: "retry" },
  build_failed: { headline: "Build failed", severity: "critical", action: "retry" },
  build_hard_failure: { headline: "Build failed", severity: "critical", action: "retry" },
  session_transport: { headline: "Terminal transport problem", severity: "critical", action: "view_pane" },
  session_unreachable: { headline: "Session unreachable", severity: "critical", action: "view_pane" },
  pane_exited: { headline: "Worker stopped", detail: "Open the session to inspect the worker and its terminal output.", severity: "critical", action: "view_pane" },
  awaiting_merge: { headline: "Change is ready to merge", severity: "warning", action: "open_pr" },
  retry: { headline: "Operation will retry", severity: "warning", action: "retry" },
  retry_loop: { headline: "Operation is retrying", severity: "warning", action: "retry" },
  retrying: { headline: "Operation is retrying", severity: "warning", action: "retry" },
  hub_disconnected: { headline: "Hub connection lost", severity: "warning", action: "retry" },
  reconnecting: { headline: "Reconnecting to hub", severity: "warning", action: "retry" },
  connection_degraded: { headline: "Connection degraded", severity: "warning", action: "none" },
  degraded_connection: { headline: "Connection degraded", severity: "warning", action: "none" },
  config_drift: { headline: "Configuration changed", severity: "warning", action: "none" },
  checkpoint: { headline: "Checkpoint saved", severity: "info", action: "none" },
  checkpoint_recorded: { headline: "Checkpoint saved", severity: "info", action: "none" },
  session_removed: { headline: "Session ended", detail: "Open the machine to confirm whether the session was intentionally removed.", severity: "info", action: "none" },
  session_added: { headline: "Session started", severity: "info", action: "none" },
  pane_added: { headline: "Worker started", severity: "info", action: "view_pane" },
  pane_removed: { headline: "Worker removed", severity: "info", action: "none" },
  controller_changed: { headline: "Terminal control changed", severity: "info", action: "none" },
  task_summary: { headline: "Task updated", severity: "info", action: "none" },
  progress_note: { headline: "Progress updated", severity: "info", action: "none" },
};

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

function clampDetail(value: string): string {
  const clean = value.replace(/\s+/g, " ").trim();
  return clean.length <= MAX_DETAIL_LENGTH ? clean : `${clean.slice(0, MAX_DETAIL_LENGTH - 1).trimEnd()}…`;
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
  const deterministic = MACHINE_EVENT_TEMPLATES[normalized]?.severity ?? "info";
  const candidate = severityFromStored(proposed);
  return candidate && SEVERITY_RANK[candidate] < SEVERITY_RANK[deterministic] ? candidate : deterministic;
}

export function actionForEvent(kind: string): AttentionAction {
  return MACHINE_EVENT_TEMPLATES[normalizedKind(kind)]?.action ?? "none";
}

export function hasDeterministicAttention(kind: string): boolean {
  return MACHINE_EVENT_TEMPLATES[normalizedKind(kind)] !== undefined;
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

function rawEventTitle(kind: string, diagnostic: unknown): string {
  if (typeof diagnostic === "string" && diagnostic.trim()) return clampSummary(diagnostic.split(/\r?\n/, 1)[0]!);
  const record = asRecord(diagnostic);
  for (const key of ["title", "summary", "message", "error", "reason"]) {
    const value = record?.[key];
    if (typeof value === "string" && value.trim()) return clampSummary(value.split(/\r?\n/, 1)[0]!);
  }
  return sentenceCase(normalizedKind(kind));
}

export function machineEventAttention(kind: string, diagnostic: unknown, enrichmentPending = false): AttentionContent {
  const normalized = normalizedKind(kind);
  if (normalized === "daemon_disconnected") return { ...daemonAttention(diagnostic), enrichmentPending: false };
  const template = MACHINE_EVENT_TEMPLATES[normalized];
  if (template) return { ...template, payload: diagnostic, enrichmentPending: false };
  return {
    headline: rawEventTitle(kind, diagnostic),
    severity: "info",
    action: "none",
    payload: diagnostic,
    enrichmentPending,
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
  const storedSummary = /^[a-z0-9_]+$/i.test(stored.trim()) ? sentenceCase(stored) : stored;
  const headline = task?.[2]
    ?? (looksLikePayload(stored) ? fallback : storedSummary);

  return {
    headline: clampSummary(headline || fallback),
    detail: item.detail,
    cause: item.cause,
    severity,
    action: item.action ?? actionForEvent(item.kind),
    ticketId: item.ticketId ?? task?.[1].toLowerCase(),
    payload: item.payload,
    fingerprint: item.fingerprint,
    enrichmentPending: item.enrichmentPending,
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
    enrichmentPending: normalized.enrichmentPending,
  };
}

/** Apply model output to the same durable card while enforcing deterministic safety floors. */
export function applyAttentionEnrichment(
  item: AttentionItem,
  enrichment: AttentionEnrichment,
  enrichedAt = new Date().toISOString(),
): AttentionItem {
  return {
    ...item,
    message: clampSummary(enrichment.summary),
    headline: clampSummary(enrichment.summary),
    detail: enrichment.detail ? clampDetail(enrichment.detail) : undefined,
    severity: severityForEvent(item.kind, enrichment.severity),
    action: enrichment.action,
    fingerprint: enrichment.fingerprint.trim(),
    enrichmentPending: false,
    enrichedAt,
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

export interface AttentionSummary {
  severity: AttentionSeverity | "clear";
  total: number;
  label: string;
  description: string;
}

/**
 * One labelled figure for the collapsed rail. Three bare numbers in three
 * colours say nothing about what they count and force two badge treatments
 * into a 48px row; the rail states the total and takes its colour from the
 * worst outstanding severity, and the per-severity split stays in the panel.
 */
export function attentionSummary(counts: AttentionCounts): AttentionSummary {
  const total = counts.critical + counts.warning + counts.info;
  if (total === 0) {
    return { severity: "clear", total: 0, label: "Clear", description: "Nothing needs attention" };
  }
  const severity: AttentionSeverity = counts.critical > 0 ? "critical" : counts.warning > 0 ? "warning" : "info";
  return {
    severity,
    total,
    label: `Needs ${total}`,
    description: `${total} need attention: ${counts.critical} critical, ${counts.warning} warning, ${counts.info} info`,
  };
}

export function dismissableInfoItems(items: readonly AttentionItem[]): AttentionItem[] {
  return items.filter((item) => !item.acknowledgedAt && severityForEvent(item.kind, item.severity) === "info");
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
