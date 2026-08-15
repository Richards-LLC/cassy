export type ConnectionStage = "idle" | "resolving" | "dialing" | "auth" | "attaching" | "live";
export type ConnectionPhase = ConnectionStage | "failed" | "backoff";

export interface ConnectionSnapshot {
  phase: ConnectionPhase;
  stage: ConnectionStage;
  since: number;
  attempt: number;
  reason?: string;
  retryInMs?: number;
  latencyMs?: number;
  missedHeartbeats: number;
  degraded: boolean;
  authFailure?: "expired" | "revoked" | "scope-mismatch";
}

export interface AttachSnapshot extends ConnectionSnapshot {
  session: string;
}

export const STAGE_TIMEOUT_MS: Readonly<Record<Exclude<ConnectionStage, "idle" | "live">, number>> = {
  resolving: 3_000,
  dialing: 5_000,
  auth: 3_000,
  attaching: 3_000,
};

export const HEARTBEAT_INTERVAL_MS = 5_000;
export const DEGRADED_AFTER_MISSED_HEARTBEATS = 2;
export const RECONNECT_AFTER_MISSED_HEARTBEATS = 4;

export function backoffDelay(attempt: number, random = Math.random): number {
  const base = Math.min(30_000, 1_000 * 2 ** Math.max(0, attempt));
  const jitter = 0.8 + random() * 0.4;
  return Math.round(base * jitter);
}

export function stageFailureDetail(stage: ConnectionStage, target: string, reason: string): string {
  const prefix = stage === "dialing"
    ? `Stuck dialing ${target} — node may be offline`
    : stage === "resolving"
      ? `Stuck resolving ${target}`
      : stage === "auth"
        ? `Authentication failed for ${target}`
        : stage === "attaching"
          ? `Terminal attach failed for ${target}`
          : `Connection failed for ${target}`;
  return `${prefix}: ${reason}`;
}

export function elapsedSeconds(snapshot: ConnectionSnapshot, now = Date.now()): number {
  return Math.max(0, Math.floor((now - snapshot.since) / 1_000));
}
