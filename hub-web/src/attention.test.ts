import { describe, expect, it } from "vitest";
import { attentionContent, daemonAttention, groupAttention, machineEventAttention } from "./attention";
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

describe("Commander ATTENTION presentation", () => {
  it("renders a real daemon diagnostic envelope as labeled human fields", () => {
    const content = daemonAttention({
      cause: { kind: "transport_lost" },
      next_action: "The daemon process is still alive with the expected process-start fingerprint; inspect the local transport and daemon log before reconnecting.",
    });

    expect(content).toEqual({
      headline: "Daemon connection lost",
      cause: "Transport Lost",
      detail: "The daemon process is still alive with the expected process-start fingerprint; inspect the local transport and daemon log before reconnecting.",
      severity: "incident",
    });
    expect(JSON.stringify(content)).not.toContain("next_action");
  });

  it("upgrades already-persisted raw daemon JSON without showing the envelope", () => {
    const content = attentionContent(item({
      message: 'soundwave / gabber-studio-sturdy-cardinal-20: Daemon ended: {"cause":{"kind":"transport_lost"},"next_action":"Inspect the local transport."}',
    }));

    expect(content).toMatchObject({ headline: "Daemon connection lost", detail: "Inspect the local transport.", severity: "incident" });
    expect(content.headline).not.toContain("{");
  });

  it("keeps task meaning ahead of its small identifier and separates incidents from notices", () => {
    const task = attentionContent(item({ kind: "blocked", message: "soundwave / gabber-studio-sturdy-cardinal-20: cas-87e7: Nothing advances an epic branch after its children merge to main" }));
    const incident = attentionContent(item({ kind: "session_transport", message: "terminal transport error" }));

    expect(task).toEqual({ headline: "Nothing advances an epic branch after its children merge to main", severity: "notice", ticketId: "cas-87e7" });
    expect(incident.severity).toBe("incident");
  });

  it("only strips this item's exact machine and session context", () => {
    const content = attentionContent(item({
      kind: "blocked",
      message: "soundwave / a-different-session: cas-87e7: Keep the original context",
    }));

    expect(content.headline).toContain("Soundwave / A-Different-Session:");
    expect(content.ticketId).toBeUndefined();
  });

  it("does not mistake arbitrary hyphenated labels or colon prose for task ids", () => {
    const hyphenated = attentionContent(item({ kind: "blocked", message: "worker-3: Keep this headline intact" }));
    const colonProse = attentionContent(item({ kind: "blocked", message: "Recovery note: preserve everything after this colon" }));

    expect(hyphenated.headline).toContain("Worker-3:");
    expect(hyphenated.ticketId).toBeUndefined();
    expect(colonProse.headline).toContain("Recovery Note:");
    expect(colonProse.ticketId).toBeUndefined();
  });

  it("is idempotent for items already normalized at creation", () => {
    const stale = item({
      kind: "blocked",
      message: "soundwave / gabber-studio-sturdy-cardinal-20: cas-fc6fa: Keep the human title",
    });
    const once = attentionContent(stale);
    const twice = attentionContent({ ...stale, message: once.headline, ...once });

    expect(twice).toEqual(once);
  });

  it("gives an exited worker an actionable incident without exposing its wire event name", () => {
    expect(machineEventAttention("pane_exited", undefined)).toEqual({
      headline: "Worker stopped",
      detail: "Open the session to inspect the worker and its terminal output.",
      severity: "incident",
    });
  });

  it("groups a real queue by machine and session for one-tap acknowledgement", () => {
    const groups = groupAttention([
      item({ id: "incident" }),
      item({ id: "task", kind: "blocked", message: "cas-87e7: Routine task" }),
      item({ id: "other", session: "cas-src-brave-finch-54" }),
    ]);

    expect(groups).toHaveLength(2);
    expect(groups[0]).toMatchObject({ machineLabel: "soundwave", session: "gabber-studio-sturdy-cardinal-20" });
    expect(groups[0].items.map((queued) => queued.id)).toEqual(["incident", "task"]);
  });
});
