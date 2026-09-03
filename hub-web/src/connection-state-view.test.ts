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
    const state = snapshot({ since: startedAt + 14_000, attachSince: startedAt, stage: "dialing", phase: "backoff" });
    expect(connectingView(state, startedAt + 15_000)).toMatchObject({
      elapsedLabel: "15s",
      actionsAvailable: true,
    });
  });

  it("keeps the machine-level clock running across retry transitions", () => {
    // D3: every transition rewrites `since`, so a machine that fails and
    // retries every second reported "0s" forever and the 5s and 15s
    // disclosures never fired.
    const machine: ConnectionSnapshotView = {
      phase: "backoff",
      stage: "attaching",
      since: startedAt + 15_800,
      connectingSince: startedAt,
      attempt: 6,
      missedHeartbeats: 0,
      degraded: false,
      reason: "Terminal attach failed for hub.example: AbortSignal.any is not a function",
    };
    expect(elapsedSeconds(machine, startedAt + 16_000)).toBe(16);
    expect(connectingView(machine, startedAt + 16_000)).toMatchObject({
      elapsedLabel: "16s",
      step: "Terminal attach failed for hub.example: AbortSignal.any is not a function",
      actionsAvailable: true,
    });
  });

  it("shows a non-retryable failure and its escape hatch immediately, not after 15s", () => {
    // Waiting 15 seconds to reveal an error that can never resolve itself is
    // 15 seconds of lying about progress.
    const fatal = snapshot({ phase: "failed", fatal: true, reason: "This browser cannot open the terminal stream." });
    expect(connectingView(fatal, startedAt + 200)).toEqual({
      elapsedSeconds: 0,
      elapsedLabel: "0s",
      step: "This browser cannot open the terminal stream.",
      actionsAvailable: true,
    });
  });

  it("formats long-running attempts without introducing another state clock", () => {
    expect(connectingView(snapshot(), startedAt + 72_000).elapsedLabel).toBe("1m 12s");
  });

  it("promises no next attempt on a retained frame when nothing is retrying", () => {
    const fatal = snapshot({ phase: "failed", fatal: true, attempt: 2, retryInMs: 4_000 });
    expect(disconnectedView(fatal, startedAt + 4_000).retryLabel).toBe("not retrying");
    expect(shouldRetainDisconnectedFrame(fatal)).toBe(true);
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
