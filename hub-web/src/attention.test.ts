import { describe, expect, it } from "vitest";
import {
  attentionContent,
  attentionCounts,
  applyAttentionEnrichment,
  coalesceAttention,
  createAttentionItem,
  daemonAttention,
  dismissableInfoItems,
  groupAttention,
  machineEventAttention,
  severityForEvent,
} from "./attention";
import { cycleAttentionGroup } from "./attention-view";
import type { AttentionItem } from "./types";

const createdAt = "2026-08-15T02:30:00Z";

function item(overrides: Partial<AttentionItem>): AttentionItem {
  return {
    id: "attention-1",
    machineId: "soundwave",
    machineLabel: "soundwave",
    session: "gabber-studio-sturdy-cardinal-20",
    kind: "daemon_disconnected",
    message: "Daemon connection lost",
    createdAt,
    ...overrides,
  };
}

describe("Commander attention triage queue", () => {
  it("renders a daemon envelope as human fields while retaining payload for Details", () => {
    const diagnostic = {
      cause: { kind: "transport_lost" },
      next_action: "Inspect the local transport before reconnecting.",
    };
    const content = daemonAttention(diagnostic);

    expect(content).toMatchObject({
      headline: "Daemon connection lost",
      cause: "Transport Lost",
      detail: "Inspect the local transport before reconnecting.",
      severity: "critical",
      action: "retry",
      payload: diagnostic,
    });
    expect(content.headline).not.toContain("next_action");
  });

  it("re-derives stale prefixed daemon JSON at render time", () => {
    const content = attentionContent(item({
      message: 'soundwave / gabber-studio-sturdy-cardinal-20: Daemon ended: {"cause":{"kind":"transport_lost"},"next_action":"Inspect the local transport."}',
    }));

    expect(content).toMatchObject({ headline: "Daemon connection lost", detail: "Inspect the local transport.", severity: "critical" });
    expect(content.headline).not.toMatch(/[{}]|next_action|soundwave/i);
  });

  it("strips only this record's exact context and a real CAS ticket prefix", () => {
    const task = attentionContent(item({
      kind: "progress_note",
      message: "soundwave / gabber-studio-sturdy-cardinal-20: cas-87e7: Epic branch advanced",
    }));
    const otherContext = attentionContent(item({
      kind: "progress_note",
      message: "soundwave / a-different-session: cas-87e7: Keep the original context",
    }));

    expect(task).toMatchObject({ headline: "Epic branch advanced", severity: "info", ticketId: "cas-87e7" });
    expect(otherContext.headline).toContain("soundwave / a-different-session:");
    expect(otherContext.ticketId).toBeUndefined();
  });

  it("keeps a hub event stream ticket on the derived card content", () => {
    const [card] = coalesceAttention([item({
      kind: "task_summary",
      message: "cas-87e7: Epic branch advanced",
      ticketId: undefined,
    })]);

    // Hub event-stream records derive the ticket while the stored event itself
    // stays unchanged. Rendering must therefore use card.content.ticketId.
    expect(card.latest.ticketId).toBeUndefined();
    expect(card.content.ticketId).toBe("cas-87e7");
  });

  it("never promotes arbitrary JSON into the summary line", () => {
    const content = attentionContent(item({
      kind: "config_drift",
      headline: undefined,
      message: '{"expected":"enabled","actual":"disabled"}',
    }));

    expect(content).toMatchObject({ headline: "Config Drift", severity: "warning" });
    expect(content.headline).not.toContain("{");
  });

  it.each([
    ["daemon_ended", "critical"],
    ["auth_blocked", "critical"],
    ["ci_failed", "critical"],
    ["session_unreachable", "critical"],
    ["AwaitingMerge", "warning"],
    ["retry_loop", "warning"],
    ["connection_degraded", "warning"],
    ["config_drift", "warning"],
    ["checkpoint_recorded", "info"],
    ["task_summary", "info"],
    ["progress_note", "info"],
  ] as const)("maps %s deterministically to %s at ingestion", (kind, severity) => {
    expect(severityForEvent(kind)).toBe(severity);
    expect(createAttentionItem({
      id: kind,
      machineId: "soundwave",
      machineLabel: "soundwave",
      session: "session",
      kind,
      createdAt,
    }, "Human summary").severity).toBe(severity);
  });

  it("allows an enriched severity upgrade but never a deterministic-critical downgrade", () => {
    expect(severityForEvent("progress_note", "warning")).toBe("warning");
    expect(severityForEvent("daemon_ended", "info")).toBe("critical");
  });

  it("patches the same provisional record while enforcing schema bounds and the severity floor", () => {
    const provisional = createAttentionItem({
      id: "event-7",
      machineId: "soundwave",
      machineLabel: "soundwave",
      session: "session",
      kind: "daemon_ended",
      createdAt,
    }, { ...machineEventAttention("daemon_ended", undefined), enrichmentPending: true });
    const enriched = applyAttentionEnrichment(provisional, {
      severity: "info",
      summary: "Daemon crashed while testing the auth refactor ".repeat(4),
      detail: "auth.rs:44 serde_json panic ".repeat(8),
      action: "retry",
      fingerprint: " auth.rs-serde-panic ",
    }, "2026-08-15T03:00:00Z");

    expect(enriched).toMatchObject({
      id: "event-7",
      severity: "critical",
      action: "retry",
      fingerprint: "auth.rs-serde-panic",
      enrichmentPending: false,
      enrichedAt: "2026-08-15T03:00:00Z",
    });
    expect(enriched.headline!.length).toBeLessThanOrEqual(90);
    expect(enriched.detail!.length).toBeLessThanOrEqual(120);
  });

  it("renders unknown free text immediately as one-line provisional info", () => {
    const content = machineEventAttention("daemon_error", "serde panic in auth.rs:44\n{\"secret\":true}", true);

    expect(content).toMatchObject({
      headline: "serde panic in auth.rs:44",
      severity: "info",
      action: "none",
      enrichmentPending: true,
    });
  });

  it("sorts critical, warning, info and newest-first within each level", () => {
    const cards = coalesceAttention([
      item({ id: "info", kind: "progress_note", createdAt: "2026-08-15T03:00:00Z", message: "Progress" }),
      item({ id: "critical-old", kind: "daemon_ended", createdAt: "2026-08-15T02:00:00Z", message: "Daemon stopped" }),
      item({ id: "warning", kind: "config_drift", createdAt: "2026-08-15T04:00:00Z", message: "Config changed" }),
      item({ id: "critical-new", kind: "ci_failed", createdAt: "2026-08-15T05:00:00Z", message: "CI failed" }),
    ]);

    expect(cards.map((card) => card.latest.id)).toEqual(["critical-new", "critical-old", "warning", "info"]);
  });

  it("coalesces identical repeats, bumps ×N, and uses the latest recurrence for ordering", () => {
    const cards = coalesceAttention([
      item({ id: "first", kind: "retry_loop", createdAt: "2026-08-15T02:00:00Z", message: "Retrying attach" }),
      item({ id: "other", kind: "config_drift", createdAt: "2026-08-15T02:30:00Z", message: "Config changed" }),
      item({ id: "repeat", kind: "retry_loop", createdAt: "2026-08-15T03:00:00Z", message: "Retrying attach" }),
    ]);

    expect(cards).toHaveLength(2);
    expect(cards[0]).toMatchObject({ count: 2, latest: { id: "repeat" } });
    expect(cards[0].items.map((queued) => queued.id)).toEqual(["first", "repeat"]);
  });

  it("coalesces differently-worded failures after enrichment supplies one stable fingerprint", () => {
    const first = applyAttentionEnrichment(item({ id: "first", kind: "daemon_error", message: "thread panicked in auth" }), {
      severity: "critical", summary: "Authentication worker crashed", detail: "auth.rs:44", action: "retry", fingerprint: "auth.rs-serde-panic",
    });
    const second = applyAttentionEnrichment(item({ id: "second", kind: "daemon_error", message: "serde unwrap terminated daemon", createdAt: "2026-08-15T03:00:00Z" }), {
      severity: "critical", summary: "Daemon stopped parsing authentication", detail: "serde_json error", action: "retry", fingerprint: "auth.rs-serde-panic",
    });

    const cards = coalesceAttention([first, second]);
    expect(cards).toHaveLength(1);
    expect(cards[0]).toMatchObject({ count: 2, latest: { id: "second" } });
  });

  it("groups consecutive same-session cards and bounds a 20+ event queue to six visible groups", () => {
    const events = Array.from({ length: 24 }, (_, index) => item({
      id: `event-${index}`,
      machineId: `machine-${index}`,
      machineLabel: `machine-${index}`,
      session: `session-${index}`,
      kind: "progress_note",
      message: `Progress ${index}`,
      createdAt: `2026-08-15T03:${String(index).padStart(2, "0")}:00Z`,
    }));
    const groups = groupAttention(events);

    expect(groups).toHaveLength(6);
    expect(groups.at(-1)).toMatchObject({ overflow: true });
    expect(groups.reduce((total, group) => total + group.count, 0)).toBe(24);
  });

  it("counts outstanding events by severity even when repeats coalesce", () => {
    const events = [
      item({ id: "critical-a", kind: "daemon_ended" }),
      item({ id: "critical-b", kind: "daemon_ended" }),
      item({ id: "warning", kind: "config_drift" }),
      item({ id: "info", kind: "progress_note" }),
      item({ id: "dismissed", kind: "progress_note", acknowledgedAt: createdAt }),
    ];
    expect(attentionCounts(events)).toEqual({ critical: 2, warning: 1, info: 1 });
  });

  it("returns only outstanding info items for bulk dismissal", () => {
    const events = [
      item({ id: "info", kind: "progress_note" }),
      item({ id: "acknowledged", kind: "progress_note", acknowledgedAt: createdAt }),
      item({ id: "warning", kind: "config_drift" }),
    ];

    expect(dismissableInfoItems(events).map((event) => event.id)).toEqual(["info"]);
  });

  it("cycles rendered attention group toggles in either direction", () => {
    const focused: string[] = [];
    const toggles = ["first", "second", "third"].map((id) => ({
      id,
      focus() { focused.push(id); ownerDocument.activeElement = this; },
      scrollIntoView() { /* exercised by the keyboard API */ },
    }));
    const ownerDocument = { activeElement: toggles[0] };
    for (const toggle of toggles) Object.assign(toggle, { ownerDocument });
    const container = {
      ownerDocument,
      querySelectorAll: () => toggles,
    } as unknown as HTMLElement;

    expect(cycleAttentionGroup(container, 1)?.id).toBe("second");
    expect(cycleAttentionGroup(container, -1)?.id).toBe("first");
    expect(focused).toEqual(["second", "first"]);
  });

  it("gives an exited worker an actionable critical card without exposing the wire name", () => {
    expect(machineEventAttention("pane_exited", undefined)).toMatchObject({
      headline: "Worker stopped",
      detail: "Open the session to inspect the worker and its terminal output.",
      severity: "critical",
      action: "view_pane",
    });
  });
});
