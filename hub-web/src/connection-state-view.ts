export type ConnectionStage = "idle" | "resolving" | "dialing" | "auth" | "attaching" | "live";
export type ConnectionPhase = ConnectionStage | "failed" | "backoff";

/**
 * Structural consumer view of HubConnectionSupervisor's connection snapshot.
 * Keeping this interface at the view seam lets the lifecycle own all state and
 * timing while the UI owns only presentation thresholds.
 */
export interface ConnectionSnapshotView {
  readonly phase: ConnectionPhase;
  readonly stage: ConnectionStage;
  readonly since: number;
  readonly attempt: number;
  readonly reason?: string;
  readonly retryInMs?: number;
  readonly latencyMs?: number;
  readonly missedHeartbeats: number;
  readonly degraded: boolean;
  readonly authFailure?: "expired" | "revoked" | "scope-mismatch";
}

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

const STAGE_COPY: Record<ConnectionStage, string> = {
  idle: "waiting to start the connection",
  resolving: "resolving the target node",
  dialing: "dialing the relay",
  auth: "checking session authorization",
  attaching: "waiting for relay handshake",
  live: "waiting for the terminal heartbeat",
};

export function elapsedSeconds(snapshot: Pick<ConnectionSnapshotView, "since">, now = Date.now()): number {
  return Math.max(0, Math.floor((now - snapshot.since) / 1_000));
}

export function formatElapsed(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return minutes > 0 ? `${minutes}m ${String(remainder).padStart(2, "0")}s` : `${remainder}s`;
}

export function connectingView(snapshot: ConnectionSnapshotView, now = Date.now()): ConnectingView {
  const elapsed = elapsedSeconds(snapshot, now);
  return {
    elapsedSeconds: elapsed,
    elapsedLabel: formatElapsed(elapsed),
    step: elapsed >= 5 ? snapshot.reason ?? STAGE_COPY[snapshot.stage] : undefined,
    actionsAvailable: elapsed >= 15,
  };
}

export function disconnectedView(snapshot: ConnectionSnapshotView, now = Date.now()): DisconnectedView {
  const elapsed = elapsedSeconds(snapshot, now);
  const retrySeconds = snapshot.retryInMs === undefined ? undefined : Math.max(0, Math.ceil(snapshot.retryInMs / 1_000));
  return {
    elapsedSeconds: elapsed,
    attempt: Math.max(1, snapshot.attempt),
    retryLabel: retrySeconds === undefined ? "reconnecting" : `reconnecting in ${retrySeconds}s`,
  };
}

export function shouldRetainDisconnectedFrame(snapshot: ConnectionSnapshotView): boolean {
  return snapshot.degraded || snapshot.phase === "failed" || snapshot.phase === "backoff";
}
