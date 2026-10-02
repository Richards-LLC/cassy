import { describe, expect, it } from "vitest";
import {
  attachElapsedSeconds,
  backoffDelay,
  connectingAnchor,
  DEGRADED_AFTER_MISSED_HEARTBEATS,
  elapsedSeconds,
  headerConnectionChip,
  RECONNECT_AFTER_MISSED_HEARTBEATS,
  stageFailureDetail,
  STAGE_TIMEOUT_MS,
  type ConnectionSnapshot,
} from "./connection-state";

describe("Commander connection lifecycle contract", () => {
  it("pins the guide's per-stage deadlines and heartbeat thresholds", () => {
    expect(STAGE_TIMEOUT_MS).toEqual({ resolving: 3_000, dialing: 5_000, auth: 3_000, attaching: 3_000 });
    expect(DEGRADED_AFTER_MISSED_HEARTBEATS).toBe(2);
    expect(RECONNECT_AFTER_MISSED_HEARTBEATS).toBe(4);
  });

  it("carries one connecting anchor across retries and drops it once live", () => {
    const failing: ConnectionSnapshot = {
      phase: "backoff", stage: "attaching", since: 1_000, connectingSince: 1_000, attempt: 1, missedHeartbeats: 0, degraded: false,
    };
    // A retry keeps the original anchor, so the overlay clock advances.
    expect(connectingAnchor(failing, "resolving", 9_000)).toBe(1_000);
    expect(elapsedSeconds({ ...failing, since: 9_000 }, 16_000)).toBe(15);
    // Reaching live, or idling, ends the stretch.
    expect(connectingAnchor(failing, "live", 9_000)).toBeUndefined();
    expect(connectingAnchor(failing, "idle", 9_000)).toBeUndefined();
    // A fresh outage after a live period starts a new anchor at that moment.
    const live: ConnectionSnapshot = { phase: "live", stage: "live", since: 5_000, attempt: 0, missedHeartbeats: 0, degraded: false };
    expect(connectingAnchor(live, "dialing", 9_000)).toBe(9_000);
    expect(connectingAnchor(undefined, "resolving", 9_000)).toBe(9_000);
    // Without an anchor the reading falls back to the stage clock.
    expect(elapsedSeconds({ ...live, phase: "failed", since: 9_000 }, 12_000)).toBe(3);
  });

  it("backs off exponentially with bounded jitter and a 30 second ceiling", () => {
    expect([0, 1, 2, 3, 4, 5, 6].map((attempt) => backoffDelay(attempt, () => 0.5)))
      .toEqual([1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]);
    expect(backoffDelay(2, () => 0)).toBe(3_200);
    expect(backoffDelay(2, () => 1)).toBe(4_800);
  });

  it("names the failed stage and target instead of reporting a generic timeout", () => {
    expect(stageFailureDetail("dialing", "100.64.0.8:4173", "timed out after 5s"))
      .toContain("Stuck dialing 100.64.0.8:4173 — node may be offline");
    expect(stageFailureDetail("attaching", "studio/session-a", "no Welcome after 3s"))
      .toContain("Terminal attach failed for studio/session-a");
  });

  it("reports elapsed stage time from the transition timestamp", () => {
    const snapshot: ConnectionSnapshot = {
      phase: "dialing", stage: "dialing", since: 10_000, attempt: 0, missedHeartbeats: 0, degraded: false,
    };
    expect(elapsedSeconds(snapshot, 17_900)).toBe(7);
  });

  it("reports total attach time independently of the current stage", () => {
    expect(attachElapsedSeconds({
      session: "factory-a", phase: "backoff", stage: "attaching", since: 18_000,
      attachSince: 1_000, attempt: 3, missedHeartbeats: 0, degraded: false,
    }, 21_500)).toBe(20);
  });
});

describe("Terminal view header connection chip (journey F17, cas-bf07 QA)", () => {
  const live = (update: Partial<ConnectionSnapshot> = {}): ConnectionSnapshot => ({
    phase: "live", stage: "live", since: 0, attempt: 0, missedHeartbeats: 0, degraded: false, ...update,
  });

  it("is neutral and says Checking… before the first latency sample, never a green dot", () => {
    expect(headerConnectionChip(live(), "live", "Live")).toEqual({ state: "checking", text: "Checking…" });
    expect(headerConnectionChip(undefined, "idle", "Idle")).toEqual({ state: "checking", text: "Checking…" });
    // One missed heartbeat clears the sample; not degraded yet.
    expect(headerConnectionChip(live({ missedHeartbeats: 1 }), "live", "Live")).toEqual({ state: "checking", text: "Checking…" });
  });

  it("shows the latency beside the attach state once a sample lands", () => {
    expect(headerConnectionChip(live({ latencyMs: 41 }), "live", "Live")).toEqual({ state: "live", text: "41ms" });
  });

  it("names a degraded machine with the amber dot even while the terminal is still attached (QA F01)", () => {
    const degraded = live({ missedHeartbeats: DEGRADED_AFTER_MISSED_HEARTBEATS, degraded: true });
    expect(headerConnectionChip(degraded, "live", "Unsteady")).toEqual({ state: "degraded", text: "Unsteady" });
    expect(headerConnectionChip({ ...degraded, latencyMs: 12 }, "live", "Unsteady")).toEqual({ state: "degraded", text: "Unsteady" });
  });

  it("names a machine that is not live by its own phase and label", () => {
    expect(headerConnectionChip(live({ phase: "backoff", stage: "dialing" }), "live", "Reconnecting")).toEqual({ state: "backoff", text: "Reconnecting" });
  });

  it("never says Status unavailable", () => {
    for (const machine of [undefined, live(), live({ degraded: true, missedHeartbeats: 3 }), live({ phase: "failed" })]) {
      expect(headerConnectionChip(machine, "live", "Unreachable").text).not.toMatch(/unavailable/i);
    }
  });
});
