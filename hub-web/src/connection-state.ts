export type ConnectionStage = "idle" | "resolving" | "dialing" | "auth" | "attaching" | "live";
export type ConnectionPhase = ConnectionStage | "failed" | "backoff";

export interface ConnectionSnapshot {
  phase: ConnectionPhase;
  stage: ConnectionStage;
  since: number;
  /**
   * Start of the uninterrupted not-live lifecycle, stable across retries.
   * `since` is rewritten by every stage transition, so a machine that fails
   * and retries every second reported "0s" forever and the connect overlay's
   * 5s and 15s disclosures never fired (report cas-b652, defect D3).
   */
  connectingSince?: number;
  /** A failure that retrying cannot fix; the UI must say so instead of spinning. */
  fatal?: boolean;
  attempt: number;
  reason?: string;
  retryInMs?: number;
  latencyMs?: number;
  missedHeartbeats: number;
  degraded: boolean;
  authFailure?: "expired" | "revoked" | "scope-mismatch" | "needs-pairing";
}

export interface AttachSnapshot extends ConnectionSnapshot {
  session: string;
  /** Start of the uninterrupted not-live lifecycle; stable across retries. */
  attachSince?: number;
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
  return Math.max(0, Math.floor((now - (snapshot.connectingSince ?? snapshot.since)) / 1_000));
}

/**
 * The anchor a transition should carry forward: unchanged while the lifecycle
 * stays out of live, cleared once it is live or idle.
 */
export function connectingAnchor(previous: ConnectionSnapshot | undefined, phase: ConnectionPhase, now: number): number | undefined {
  if (phase === "live" || phase === "idle") return undefined;
  const continuing = previous && previous.phase !== "live" && previous.phase !== "idle";
  return continuing ? (previous.connectingSince ?? previous.since) : now;
}

export function attachElapsedSeconds(snapshot: AttachSnapshot, now = Date.now()): number {
  return Math.max(0, Math.floor((now - (snapshot.attachSince ?? snapshot.since)) / 1_000));
}
