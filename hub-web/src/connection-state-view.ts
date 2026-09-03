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

const STAGE_COPY: Record<ConnectionSnapshot["stage"], string> = {
  idle: "waiting to start the connection",
  resolving: "resolving the target node",
  dialing: "dialing the relay",
  auth: "checking session authorization",
  attaching: "waiting for relay handshake",
  live: "waiting for the terminal heartbeat",
};

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

export function shouldRetainDisconnectedFrame(snapshot: ConnectionSnapshotView): boolean {
  return snapshot.degraded || snapshot.phase !== "live";
}
