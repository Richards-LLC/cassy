import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";
import {
  attachElapsedSeconds,
  backoffDelay,
  DEGRADED_AFTER_MISSED_HEARTBEATS,
  elapsedSeconds,
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

  it("keeps latency absent until a heartbeat round trip measures it", async () => {
    const source = await readFile(new URL("connection.ts", import.meta.url), "utf8");
    expect(source).toContain('this.transition("live", "live");');
    expect(source).not.toContain('{ latencyMs: 0 }');
    expect(source).toContain('latencyMs: Math.round(performance.now() - started)');
    expect(source).toContain('Math.max(0, Math.round(performance.now() - this.healthPing.startedAt))');
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
