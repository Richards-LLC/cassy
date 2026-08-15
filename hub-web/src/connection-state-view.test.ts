import { describe, expect, it } from "vitest";
import {
  connectingView,
  disconnectedView,
  elapsedSeconds,
  shouldRetainDisconnectedFrame,
  type ConnectionSnapshotView,
} from "./connection-state-view";

const startedAt = Date.parse("2026-08-15T04:00:00Z");

function snapshot(overrides: Partial<ConnectionSnapshotView> = {}): ConnectionSnapshotView {
  return {
    session: "factory-a",
    phase: "attaching",
    stage: "attaching",
    since: startedAt,
    attachSince: startedAt,
    attempt: 1,
    missedHeartbeats: 0,
    degraded: false,
    ...overrides,
  };
}

describe("Commander designed connection states", () => {
  it("derives elapsed text and both disclosure thresholds from the lifecycle clock", () => {
    expect(elapsedSeconds(snapshot(), startedAt + 4_999)).toBe(4);
    expect(connectingView(snapshot(), startedAt + 4_999)).toEqual({
      elapsedSeconds: 4,
      elapsedLabel: "4s",
      step: undefined,
      actionsAvailable: false,
    });
    expect(connectingView(snapshot(), startedAt + 5_000)).toMatchObject({
      elapsedLabel: "5s",
      step: "waiting for relay handshake",
      actionsAvailable: false,
    });
    expect(connectingView(snapshot({ reason: "target node is offline" }), startedAt + 15_000)).toMatchObject({
      elapsedLabel: "15s",
      step: "target node is offline",
      actionsAvailable: true,
    });
  });

  it("keeps disclosure thresholds on total attach age when the current stage resets", () => {
    const state = snapshot({ since: startedAt + 14_000, attachSince: startedAt, stage: "backoff", phase: "backoff" });
    expect(connectingView(state, startedAt + 15_000)).toMatchObject({
      elapsedLabel: "15s",
      actionsAvailable: true,
    });
  });

  it("formats long-running attempts without introducing another state clock", () => {
    expect(connectingView(snapshot(), startedAt + 72_000).elapsedLabel).toBe("1m 12s");
  });

  it("uses lifecycle attempt and retry data for a retained disconnected frame", () => {
    const state = snapshot({ phase: "backoff", stage: "dialing", attempt: 3, retryInMs: 2_400, degraded: true });
    expect(disconnectedView(state, startedAt + 34_000)).toEqual({
      elapsedSeconds: 34,
      attempt: 3,
      retryLabel: "reconnecting in 3s",
    });
    expect(shouldRetainDisconnectedFrame(state)).toBe(true);
    expect(shouldRetainDisconnectedFrame(snapshot({ phase: "dialing", stage: "dialing" }))).toBe(true);
    expect(shouldRetainDisconnectedFrame(snapshot({ phase: "live" }))).toBe(false);
  });
});
