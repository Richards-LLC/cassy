import { describe, expect, it } from "vitest";
import type { InboxEvent } from "./store";
import { parsePresenceSnapshot, presenceLabel, presenceNotices } from "./presence";

const at = "2026-10-06T11:00:00Z";
export function snapshotBody(overrides: Record<string, unknown> = {}) {
  return { wire_version: 1, observer_status: "ok", observer_checked_at: at, machines: [{
    machine_id: "machine", hub_id: "hub", monitoring: "enabled", monitoring_generation: "1",
    presence: "observed", last_report_at: at, lease_expires_at: at, deadline_at: at,
    silence: null, open_outage: null, components: [{ component: "factory", state: "down", observed_at: at }], ...overrides,
  }] };
}

function notice(kind: "machine_unobserved" | "machine_recovered", overrides: Partial<InboxEvent> = {}): InboxEvent {
  const recovered = kind === "machine_recovered";
  return {
    accountId: "account", feedGeneration: "1", sequence: recovered ? "2" : "1", eventId: recovered ? "recovery" : "outage",
    scope: "machine", producerKind: "cloud_observer", hubId: "hub", projectId: null, sessionId: null, keyEpoch: "1", digest: "digest",
    storedAt: at, expiresAt: at, verification: "verified", failure: null, acked: true,
    plaintext: { type: "psc.operator.machine_presence", v: 1, kind, account_id: "account", machine_id: "machine", hub_id: "hub", outage_epoch: "3", ref_event_id: recovered ? "outage" : null, detected_at: at },
    ...overrides,
  };
}

describe("machine presence contract projection", () => {
  it("keeps component failure separate from a fresh lease and watchdog health", () => {
    const value = parsePresenceSnapshot(snapshotBody());
    expect(presenceLabel(value.machines[0])).toBe("Reporting to Cassy Cloud");
    expect(value.machines[0].components).toEqual([{ component: "factory", state: "down", observedAt: at }]);
    expect(parsePresenceSnapshot({ ...snapshotBody(), observer_status: "unavailable", observer_checked_at: null }).observerStatus).toBe("unavailable");
  });

  it.each(["observed", "grace", "pending_first_report", "unobserved", "silenced"])("accepts the server presence state %s without deriving a power state", (state) => {
    expect(parsePresenceSnapshot(snapshotBody({ presence: state })).machines[0].presence).toBe(state);
    expect(presenceLabel(parsePresenceSnapshot(snapshotBody({ presence: state })).machines[0])).not.toMatch(/powered off|sleeping|online/);
  });

  it("reads disabled and incapable machines with null observation fields", () => {
    const disabled = { presence: null, last_report_at: null, lease_expires_at: null, deadline_at: null, components: null };
    expect(presenceLabel(parsePresenceSnapshot(snapshotBody({ ...disabled, monitoring: "disabled" })).machines[0])).toBe("Monitoring is off");
    expect(parsePresenceSnapshot(snapshotBody({ ...disabled, monitoring: "not_capable" })).machines[0].components).toEqual([]);
  });

  it.each([
    { components: [{ component: "address", state: "up", observed_at: at }] },
    { components: [{ component: "hub", state: "healthy", observed_at: at }] },
    { components: [{ component: "hub", state: "up", observed_at: "yesterday" }] },
    { components: [{ component: "hub", state: "up", observed_at: at }, { component: "hub", state: "down", observed_at: at }] },
    { monitoring_generation: "01" }, { monitoring: "disabled", presence: "observed" },
  ])("refuses malformed/contradictory status rather than claiming healthy: %j", (overrides) => {
    expect(() => parsePresenceSnapshot(snapshotBody(overrides))).toThrow();
  });

  it("projects one verified outage and its matching recovery after repeated replay", () => {
    const outage = notice("machine_unobserved");
    const recovery = notice("machine_recovered");
    expect(presenceNotices([recovery, outage, outage])).toEqual([
      expect.objectContaining({ eventId: "outage", kind: "machine_unobserved", outageEpoch: "3", refEventId: null }),
      expect.objectContaining({ eventId: "recovery", kind: "machine_recovered", outageEpoch: "3", refEventId: "outage" }),
    ]);
  });

  it("never projects unverified, session, wrong-account or unbound recovered notices", () => {
    expect(presenceNotices([
      notice("machine_unobserved", { verification: "unverified" }),
      notice("machine_unobserved", { scope: "session" }),
      notice("machine_unobserved", { accountId: "other-account" }),
      notice("machine_recovered", { plaintext: { ...(notice("machine_recovered").plaintext as object), ref_event_id: null } }),
    ])).toEqual([]);
  });
});
