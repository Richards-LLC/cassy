// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  ATTACH_QUIET_MS,
  CONVERSATION_OPENING,
  attachInProgress,
  connectionTimeline,
  renderConnectionSurfaceInto,
  connectingView,
  disconnectedView,
  elapsedSeconds,
  lostConnectionBanner,
  outageControlsReason,
  outageRefusal,
  shouldRetainDisconnectedFrame,
  transportFailureNeedsAttention,
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
  it("renders an honest attempt timeline without a client-invented history", () => {
    expect(connectionTimeline(snapshot({ attempt: 3, phase: "backoff", reason: "hub did not answer", retryInMs: 2_400 }))).toEqual([
      { label: "Earlier attempts", detail: "2 attempts did not reach a live session", tone: "evidence" },
      { label: "Attempt 3", detail: "Retry scheduled", tone: "retry" },
      { label: "Diagnostic", detail: "hub did not answer", tone: "evidence" },
      { label: "Next attempt", detail: "reconnecting in 3s", tone: "retry" },
    ]);
    expect(connectionTimeline(snapshot({ phase: "failed", fatal: true, reason: "unsupported browser" }))).toEqual([
      { label: "Attempt 1", detail: "Connection failed", tone: "failed" },
      { label: "Outcome", detail: "unsupported browser", tone: "failed" },
    ]);
  });

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

describe("transportFailureNeedsAttention", () => {
  it("leaves a retrying drop to the banner, header, row and footer (cas-90d4)", () => {
    // The rail used to raise "Terminal transport problem" beside the plain
    // "Reconnecting…" banner, with counts that disagreed.
    expect(transportFailureNeedsAttention(snapshot({ phase: "failed", reason: "Terminal connection closed before it became ready" }))).toBe(false);
    expect(transportFailureNeedsAttention(snapshot({ phase: "backoff" }))).toBe(false);
    expect(transportFailureNeedsAttention(undefined)).toBe(false);
  });

  it("leaves a pairing loss to its machine card", () => {
    expect(transportFailureNeedsAttention(snapshot({ phase: "failed", fatal: true, authFailure: "revoked" }))).toBe(false);
  });

  it("raises a failure that will not retry", () => {
    expect(transportFailureNeedsAttention(snapshot({ phase: "failed", fatal: true, reason: "This browser cannot open the terminal stream." }))).toBe(true);
  });
});

describe("the attach surface opens calmly (journey F3)", () => {
  const JARGON = /relay|attempt|authori[sz]ation|handshake|heartbeat|resolving|dialing/i;
  const card = () => { const target = document.createElement("div"); document.body.replaceChildren(target); return target; };

  it("shows only the title during the quiet window", () => {
    const target = card();
    renderConnectionSurfaceInto(target, "patient-pelican-9", snapshot({ phase: "auth", stage: "auth" }), {}, startedAt + ATTACH_QUIET_MS - 1, { openingTitle: CONVERSATION_OPENING });
    expect(target.textContent).toBe("Opening the conversation…");
    expect(target.querySelector(".connection-timeline")).toBeNull();
    expect(target.textContent).not.toContain("patient-pelican-9");
  });

  it("then offers the attempt and stage behind a closed Details, and keeps it open across repaints", () => {
    const target = card();
    const state = snapshot({ phase: "dialing", stage: "dialing" });
    renderConnectionSurfaceInto(target, "patient-pelican-9", state, {}, startedAt + ATTACH_QUIET_MS, { openingTitle: CONVERSATION_OPENING });
    const details = target.querySelector<HTMLDetailsElement>(":scope > details.connection-details")!;
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")?.textContent).toBe("Details");
    expect(details.querySelector(".connection-timeline")?.textContent).toContain("dialing the relay");
    // Everything outside Details is free of relay vocabulary.
    expect(target.querySelector(".terminal-connecting-title")?.textContent).not.toMatch(JARGON);
    expect(target.querySelector(":scope > .connection-timeline")).toBeNull();
    details.open = true;
    renderConnectionSurfaceInto(target, "patient-pelican-9", state, {}, startedAt + 2_000, { openingTitle: CONVERSATION_OPENING });
    expect(target.querySelector<HTMLDetailsElement>(".connection-details")?.open).toBe(true);
  });

  it("keeps the terminal's title and offers Retry and Diagnose once the attach is slow", () => {
    const target = card();
    renderConnectionSurfaceInto(target, "factory-a", snapshot(), { retry: () => {}, diagnose: () => {} }, startedAt + 15_000);
    expect(target.querySelector(".terminal-connecting-title")?.textContent).toBe("Connecting to factory-a…");
    expect(target.querySelector(".connection-details")).not.toBeNull();
    expect([...target.querySelectorAll(".terminal-connecting-actions button")].map((button) => button.textContent)).toEqual(["Retry", "Diagnose"]);
  });

  it("shows a failure or a retry with its timeline in the open, not behind Details", () => {
    for (const state of [snapshot({ phase: "backoff", retryInMs: 2_000 }), snapshot({ phase: "failed", reason: "hub did not answer" }), snapshot({ phase: "failed", fatal: true })]) {
      expect(attachInProgress(state)).toBe(false);
      const target = card();
      renderConnectionSurfaceInto(target, "factory-a", state, {}, startedAt + 100, { openingTitle: CONVERSATION_OPENING });
      expect(target.querySelector(".connection-details")).toBeNull();
      expect(target.querySelector(":scope > .connection-timeline")).not.toBeNull();
      expect(target.querySelector(".terminal-connecting-title")?.textContent).not.toBe(CONVERSATION_OPENING);
    }
    expect(attachInProgress(snapshot())).toBe(true);
  });

  it("keeps a never-live conversation's first retry calm: the opening title, the retry behind Details (cas-28df)", () => {
    const target = card();
    const retry = snapshot({ phase: "backoff", stage: "attaching", attempt: 1, retryInMs: 1_000, reason: "Terminal opened but sent no session state within 3s" });
    renderConnectionSurfaceInto(target, "patient-pelican-9", retry, {}, startedAt + 3_200, { openingTitle: CONVERSATION_OPENING, quietRetry: true });
    expect(target.querySelector(".terminal-connecting-title")?.textContent).toBe(CONVERSATION_OPENING);
    expect(target.querySelector(":scope > .connection-timeline")).toBeNull();
    const details = target.querySelector<HTMLDetailsElement>(":scope > details.connection-details")!;
    expect(details.open).toBe(false);
    // The evidence is all still there for whoever asks.
    expect(details.querySelector(".connection-timeline")?.textContent).toContain("Retry scheduled");
    expect(details.querySelector(".connection-timeline")?.textContent).toContain("no session state within 3s");
    expect(target.textContent).not.toMatch(/interrupted|retrying/i);
    // A fatal failure is never quieted.
    const fatal = card();
    renderConnectionSurfaceInto(fatal, "patient-pelican-9", snapshot({ phase: "failed", fatal: true }), {}, startedAt + 3_200, { openingTitle: CONVERSATION_OPENING, quietRetry: true });
    expect(fatal.querySelector(".terminal-connecting-title")?.textContent).toBe("Connection failed — not retrying.");
  });
});

describe("one outage, one vocabulary (journey F9)", () => {
  it("words the refusal and the disabled controls the way the banner does", () => {
    expect(lostConnectionBanner("Atlas · Linux", false)).toBe("Lost connection to Atlas · Linux. Reconnecting…");
    expect(lostConnectionBanner("Atlas · Linux", true)).toBe("Lost connection to Atlas · Linux. Not retrying.");
    expect(outageRefusal("Atlas · Linux")).toBe("Not sent: lost connection to Atlas · Linux. Your message is kept; send it again when it's back.");
    expect(outageControlsReason("Atlas · Linux")).toBe("Lost connection to Atlas · Linux. Control and interrupts return when it reconnects.");
    for (const line of [outageRefusal("Atlas · Linux"), outageControlsReason("Atlas · Linux")]) {
      expect(line.toLowerCase()).toContain("lost connection to atlas · linux");
      expect(line).not.toMatch(/hub connection|session is live/);
    }
  });
});
