import type { CauseEvidence } from "./connection-diagnostics";
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
  cause?: CauseEvidence;
  nextRetryAt?: number;
  lastSuccessAt?: number;
  /** Browser permission remediation, independent of the hub's pairing. */
  networkAccessHelp?: string;
  retryInMs?: number;
  latencyMs?: number;
  missedHeartbeats: number;
  degraded: boolean;
  authFailure?: "expired" | "revoked" | "scope-mismatch" | "needs-pairing";
}

/** A machine never live in this visit whose attempts have failed; retries continue. */
export const CANT_REACH_RETRYING = "Can't reach · retrying";
export const NEEDS_PAIRING = "Needs pairing";
/** A machine connection failure that will not retry by itself. */
export const UNREACHABLE = "Unreachable";
/**
 * cas-a6f0 (journey F8/F9): heartbeats are going unanswered on a machine that
 * still reads live. Every surface uses this one word for it (header, row,
 * footer, rail, fleet), and the sentence below where there is room (Tasks
 * panel, banner, composer, Attention rail). It replaced "Degraded", which
 * said nothing about what was happening or what came next.
 */
export const UNSTEADY = "Unsteady";
export const UNSTEADY_SENTENCE = "Connection unsteady — checking…";
/**
 * cas-d043 G04: the cause is this browser, not the machine. A browser
 * permission (Local network access) blocks the connection, or the browser
 * lacks a feature Cassy Cloud needs (a fatal failure: every fatal transport
 * failure is an UnsupportedBrowserError). The status word, the footer and the
 * empty thread say so, as the banner does, instead of "Unreachable" or
 * "Can't reach · retrying", which read as the machine's fault.
 */
export const BROWSER_BLOCKED = "Blocked by browser";
export const BROWSER_UNSUPPORTED = "Browser can't connect";
export type MachineConnectionLabelState = Pick<ConnectionSnapshot, "phase" | "degraded" | "fatal" | "authFailure" | "networkAccessHelp">;

/** Shared machine words; the caller supplies this visit's live history. */
export function machineConnectionLabel(state: MachineConnectionLabelState | undefined, everConnected = true): string {
  if (!state) return "Idle";
  if (state.phase === "live") return state.degraded ? UNSTEADY : "Live";
  if (state.phase === "failed" && state.fatal === true && !state.authFailure) return BROWSER_UNSUPPORTED;
  if (state.networkAccessHelp && !state.authFailure && (state.phase === "failed" || state.phase === "backoff")) return BROWSER_BLOCKED;
  // Never live and already failed (cas-b789): name the failed attempts while retries continue.
  const retrying = state.phase === "backoff" || (state.phase === "failed" && state.fatal !== true && !state.authFailure);
  if (retrying && !everConnected) return CANT_REACH_RETRYING;
  if (state.phase === "backoff") return "Reconnecting";
  if (state.phase === "failed") return state.authFailure ? NEEDS_PAIRING : UNREACHABLE;
  return "Connecting";
}

export interface AttachSnapshot extends ConnectionSnapshot {
  session: string;
  /** Start of the uninterrupted not-live lifecycle; stable across retries. */
  attachSince?: number;
  /**
   * cas-d15c: the hub closed just this session's stream while the machine
   * socket stayed up (its daemon link dropped). Kept through the retry, and
   * cleared by any other failure or once the session is live again.
   */
  sessionOnly?: boolean;
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

/**
 * The longest the machine connection waits between reconnect attempts. The
 * exponential backoff's 30 s ceiling suits a hub that is down; a network that
 * came back without telling the page (Tailscale switched on again: no
 * `online` event) must not wait that long to be noticed (cas-0978).
 */
export const MACHINE_RETRY_CEILING_MS = 10_000;

/** How long a health ping on the machine socket may go unanswered when the
 * page has reason to doubt it (back online, woken, network changed). */
export const SOCKET_PROBE_TIMEOUT_MS = 3_000;

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
