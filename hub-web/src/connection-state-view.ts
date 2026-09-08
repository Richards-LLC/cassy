import {
  attachElapsedSeconds,
  elapsedSeconds as stageElapsedSeconds,
  type AttachSnapshot,
  type ConnectionSnapshot,
} from "./connection-state";

/**
 * Structural consumer view of HubConnectionSupervisor's connection snapshot.
 * Keeping this interface at the view seam lets the lifecycle own all state and
 * timing while the UI owns only presentation thresholds.
 */
export type ConnectionSnapshotView = ConnectionSnapshot | AttachSnapshot;

export interface ConnectingView {
  readonly elapsedSeconds: number;
  readonly elapsedLabel: string;
  readonly step?: string;
  readonly actionsAvailable: boolean;
}

export interface DisconnectedView {
  readonly elapsedSeconds: number;
  readonly attempt: number;
  readonly retryLabel: string;
}

export interface ConnectionTimelineEntry {
  readonly label: string;
  readonly detail: string;
  readonly tone: "current" | "retry" | "failed" | "evidence";
}

export interface ConnectionSurfaceActions {
  readonly retry?: () => void;
  readonly diagnose?: () => void;
  readonly repair?: () => void;
}

const STAGE_COPY: Record<ConnectionSnapshot["stage"], string> = {
  idle: "waiting to start the connection",
  resolving: "resolving the target node",
  dialing: "dialing the relay",
  auth: "checking session authorization",
  attaching: "waiting for relay handshake",
  live: "waiting for the terminal heartbeat",
};

/**
 * The lifecycle keeps the current attempt and its best diagnostic, rather than
 * a second client-side history. The presentation can still show an honest
 * annotated timeline: a count of earlier attempts, the current stage, and the
 * latest outcome without inventing timestamps or causes.
 */
export function connectionTimeline(snapshot: ConnectionSnapshotView): ConnectionTimelineEntry[] {
  const attempt = Math.max(1, snapshot.attempt);
  const entries: ConnectionTimelineEntry[] = [];
  if (attempt > 1) {
    entries.push({
      label: "Earlier attempts",
      detail: `${attempt - 1} attempt${attempt === 2 ? "" : "s"} did not reach a live session`,
      tone: "evidence",
    });
  }

  const failed = snapshot.fatal === true || snapshot.phase === "failed";
  const retrying = snapshot.phase === "backoff";
  entries.push({
    label: `Attempt ${attempt}`,
    detail: failed ? "Connection failed" : retrying ? "Retry scheduled" : STAGE_COPY[snapshot.stage],
    tone: failed ? "failed" : retrying ? "retry" : "current",
  });

  if (snapshot.reason) {
    entries.push({
      label: failed ? "Outcome" : "Diagnostic",
      detail: snapshot.reason,
      tone: failed ? "failed" : "evidence",
    });
  }
  if (retrying && snapshot.retryInMs !== undefined) {
    const seconds = Math.max(0, Math.ceil(snapshot.retryInMs / 1_000));
    entries.push({
      label: "Next attempt",
      detail: `reconnecting in ${seconds}s`,
      tone: "retry",
    });
  }
  return entries;
}

export function elapsedSeconds(snapshot: ConnectionSnapshotView, now = Date.now()): number {
  return "session" in snapshot
    ? attachElapsedSeconds(snapshot, now)
    : stageElapsedSeconds(snapshot, now);
}

export function formatElapsed(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return minutes > 0 ? `${minutes}m ${String(remainder).padStart(2, "0")}s` : `${remainder}s`;
}

export function connectingView(snapshot: ConnectionSnapshotView, now = Date.now()): ConnectingView {
  const elapsed = elapsedSeconds(snapshot, now);
  // A failure retrying cannot fix has nothing to disclose progressively: say
  // what broke and offer the escape hatch on the first frame.
  const fatal = snapshot.fatal === true;
  return {
    elapsedSeconds: elapsed,
    elapsedLabel: formatElapsed(elapsed),
    step: fatal || elapsed >= 5 ? snapshot.reason ?? STAGE_COPY[snapshot.stage] : undefined,
    actionsAvailable: fatal || elapsed >= 15,
  };
}

export function disconnectedView(snapshot: ConnectionSnapshotView, now = Date.now()): DisconnectedView {
  const elapsed = elapsedSeconds(snapshot, now);
  const retrySeconds = snapshot.retryInMs === undefined ? undefined : Math.max(0, Math.ceil(snapshot.retryInMs / 1_000));
  return {
    elapsedSeconds: elapsed,
    attempt: Math.max(1, snapshot.attempt),
    // A failure retrying cannot fix has no next attempt to promise.
    retryLabel: snapshot.fatal === true
      ? "not retrying"
      : retrySeconds === undefined ? "reconnecting" : `reconnecting in ${retrySeconds}s`,
  };
}

/**
 * Render the connection verdict and attempt timeline shared by the live app
 * and the visual-QA fixture. Lifecycle retention and selection stay in the
 * caller; this seam owns only the presentation of a not-yet-live snapshot.
 */
export function renderConnectionSurfaceInto(
  target: HTMLElement,
  session: string,
  snapshot: ConnectionSnapshotView,
  actions: ConnectionSurfaceActions = {},
  now = Date.now(),
): void {
  const document = target.ownerDocument;
  const view = connectingView(snapshot, now);
  const fatal = snapshot.fatal === true;
  target.className = `empty terminal-state terminal-connecting${fatal ? " terminal-connect-failed" : ""}`;

  const title = document.createElement("p");
  title.className = "terminal-connecting-title";
  // A spinner and a rising counter over a failure that will never resolve is
  // the D3 overlay: it reads as progress. State the outcome instead.
  title.textContent = fatal
    ? "Connection failed — not retrying."
    : snapshot.phase === "failed"
      ? snapshot.authFailure ? "Connection failed — re-pair required." : "Connection failed — retry available."
    : snapshot.phase === "backoff"
      ? "Connection interrupted — retrying."
      : `Connecting to ${session}…`;
  target.replaceChildren(title);

  const timeline = document.createElement("ol");
  timeline.className = "connection-timeline";
  timeline.setAttribute("aria-label", "Connection attempts");
  for (const entry of connectionTimeline(snapshot)) {
    const item = document.createElement("li");
    item.className = `connection-timeline-item ${entry.tone}`;
    const marker = document.createElement("span");
    marker.className = "connection-timeline-marker";
    marker.setAttribute("aria-hidden", "true");
    const content = document.createElement("div");
    const label = document.createElement("span");
    label.className = "connection-timeline-label";
    label.textContent = entry.label;
    const detail = document.createElement("span");
    detail.className = "connection-timeline-detail";
    detail.textContent = entry.detail;
    content.append(label, detail);
    item.append(marker, content);
    timeline.append(item);
  }
  target.append(timeline);

  if (view.actionsAvailable) {
    const actionRow = document.createElement("div");
    actionRow.className = "terminal-connecting-actions";
    const addAction = (label: string, action: (() => void) | undefined): void => {
      if (!action) return;
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = label;
      button.onclick = action;
      actionRow.append(button);
    };
    addAction("Retry", actions.retry);
    addAction("Diagnose", actions.diagnose);
    if (snapshot.authFailure === "revoked" || snapshot.authFailure === "scope-mismatch" || snapshot.authFailure === "needs-pairing") {
      addAction("Re-pair", actions.repair);
    }
    if (actionRow.childElementCount > 0) target.append(actionRow);
  }
}

export function shouldRetainDisconnectedFrame(snapshot: ConnectionSnapshotView): boolean {
  return snapshot.degraded || snapshot.phase !== "live";
}
