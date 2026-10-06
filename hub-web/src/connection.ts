import { installationLock, notifyInstallation } from "./installation-access";
import { catalog, installationStore } from "./storage";
import { credentialFence, type CredentialFence } from "./commander-journal";
import { anySignal } from "./abort-signals";
import { withRequestDeadline } from "./request-deadline";
import { EventRecovery } from "./event-recovery";
import { ConnectionDiagnostics, type CauseEvidence } from "./connection-diagnostics";
import { browserSupport, unsupportedBrowserNotice } from "./browser-support";
import { CoalescedRefresh } from "./catalog-refresh";
import { dpopHeaders } from "./dpop";
import { localNetworkAccessHelp } from "./local-network-access";
import type { ArtifactView, ArtifactViewResult } from "./artifact-open";
import { SessionLaunchGrantError } from "./launch-session";
import type { BrowseListing, LaunchProfiles, LaunchRequest, LaunchResult, ProjectCatalog } from "./launch-session";
import {
  backoffDelay,
  connectingAnchor,
  DEGRADED_AFTER_MISSED_HEARTBEATS,
  HEARTBEAT_INTERVAL_MS,
  MACHINE_RETRY_CEILING_MS,
  RECONNECT_AFTER_MISSED_HEARTBEATS,
  SOCKET_PROBE_TIMEOUT_MS,
  stageFailureDetail,
  STAGE_TIMEOUT_MS,
  type ConnectionPhase,
  type ConnectionSnapshot,
  type ConnectionStage,
  type AttachSnapshot,
} from "./connection-state";
import type { ConversationHistoryMessage, ConversationHistoryPage, HubSession, LeaseState, MessageQueued, OperatorNoticeResolved, OperatorReply, PaneInfo, SessionCardSummary, SessionState, StoredMachine } from "./types";

import { sessionsPath } from "./worker-visibility";
import { dormantRevealed } from "./dormant-visibility";

export type ConnectionState = ConnectionSnapshot;

function revealDormant(): boolean {
  let storage: Storage | undefined;
  let search = "";
  try { storage = globalThis.localStorage; } catch { storage = undefined; }
  try { search = globalThis.location?.search ?? ""; } catch { search = ""; }
  return dormantRevealed(search, storage);
}
export type AuthFailureKind = "expired" | "revoked" | "scope-mismatch" | "needs-pairing";

export interface HubMachineInfo {
  schema_version: number;
  version: string;
  capabilities: string[];
  /** The supervisor CLI this machine starts sessions with by default, when its hub says. */
  default_supervisor_cli?: string;
}

/**
 * What the hub said about a refused send beyond its text (cas-0653). The hub
 * answers `upstream_unavailable` with `retryable: true` when the session's
 * daemon link was missing: the message never reached the machine, so it can
 * be held and sent again, once, when the session is live again.
 */
export type MessageRejection = { code?: string; retryable?: boolean };

/** The machine-channel or legacy error object's code and retry hint. */
export function messageRejection(error: unknown, envelope?: Record<string, unknown>): MessageRejection {
  const object = typeof error === "object" && error !== null ? error as Record<string, unknown> : undefined;
  const code = typeof error === "string" ? error : typeof object?.code === "string" ? object.code : undefined;
  const retryable = object?.retryable === true || envelope?.retryable === true || code === "upstream_unavailable";
  return { ...(code === undefined ? {} : { code }), retryable };
}

export interface HubCallbacks {
  onState(state: ConnectionState): void;
  onAttachState?(session: string, state: AttachSnapshot): void;
  onLatency?(latencyMs: number): void;
  onAuthFailure?(kind: AuthFailureKind, detail: string): void;
  onCredentialRefreshed?(machine: StoredMachine): Promise<void> | void;
  onMachineInfo?(machine: HubMachineInfo | undefined): void;
  onSessions(sessions: HubSession[], freshnessThresholdSecs?: number): void;
  onMachineEvent(event: Record<string, unknown>): void;
  onSessionState(session: string, state: SessionState, scrollback?: Record<string, number[][]>, authoritativeKeyframes?: boolean): void;
  onOutput(session: string, paneId: string, data: Uint8Array): void;
  onMessageQueued?(session: string, queued: MessageQueued, fence?: CredentialFence): void;
  onOperatorMessage?(session: string, message: ConversationHistoryMessage): void;
  onMessageRejected?(session: string, clientRef: string, detail: string, rejection?: MessageRejection): void;
  onOperatorReply?(session: string, reply: OperatorReply, fence?: CredentialFence): void;
  /** A system notice delivered earlier is over (cas-e829): retire its attention item. */
  onOperatorNoticeResolved?(session: string, resolved: OperatorNoticeResolved): void;
  onConversationHistory?(session: string, page: ConversationHistoryPage, fence?: CredentialFence): void;
  /** The first history page was requested on attach; its answer is onConversationHistory. */
  onConversationHistoryRequested?(session: string): void;
  /** The session attached without durable history: no first page will come (cas-010f). */
  onConversationHistoryUnavailable?(session: string): void;
  onSessionSummary?(session: string, summary: SessionCardSummary): void;
  onPaneKeyframe(session: string, paneId: string, data: Uint8Array): void;
  onPaneSize?(session: string, paneId: string, cols: number, rows: number, authority: string): void;
  onFlowControlReset?(session: string): void;
  onSocketError(session: string, detail: string): void;
}

/**
 * A hub request the hub refused, with what it said (cas-d382, fleet-operations
 * brief): `code` is the hub's `error` field and `detail` its `detail` (or
 * `reason`), so a control can say what the hub said instead of only
 * "failed (409)". The message keeps the old wording for anything that logs it.
 */
export class HubRequestError extends Error {
  constructor(message: string, readonly status: number, readonly code?: string, readonly detail?: string, readonly body?: Readonly<Record<string, unknown>>, readonly requestId?: string) { super(message); }
}

/** Read a refused response's `{error, detail}` body; a body that is not JSON leaves both unset. */
export async function hubRequestError(method: string, path: string, response: Response): Promise<HubRequestError> {
  let body: Record<string, unknown> | undefined;
  try { body = await response.json() as Record<string, unknown>; } catch { body = undefined; }
  const code = typeof body?.error === "string" ? body.error : undefined;
  const detail = typeof body?.detail === "string" ? body.detail : typeof body?.reason === "string" ? body.reason : undefined;
  return new HubRequestError(`${method} ${path} failed (${response.status})`, response.status, code, detail, body, response.headers.get("x-cas-request-id") ?? undefined);
}

/** A refused scope self-grant, keeping its status so a 403 can offer a pairing command instead. */
export class ScopeGrantError extends HubRequestError {
  constructor(status: number, message: string, code?: string, detail?: string) { super(message, status, code, detail); }
}

export class AuthenticationError extends Error {
  constructor(readonly kind: AuthFailureKind, message: string, readonly requestId?: string) { super(message); }
}

/**
 * The hub refused a proof, not the pairing (cas-d636): a proof signed before
 * the phone slept and sent after it woke, a clock that drifted, a replayed
 * jti. A fresh proof was already tried once. This is retried like a network
 * failure, never shown as "re-pair".
 */
export class TransientAuthError extends Error {
  constructor(readonly reason: string) { super(`the hub refused a proof (${reason}); retrying`); }
}
class EventRecoveryError extends Error {
  constructor(readonly code: "event_sequence_gap" | "viewer_lagged", readonly requestId?: string) { super(code); }
}

/** What a hub 401 says about itself (cas-d636); a legacy hub says nothing. */
export interface AuthRefusal {
  reason?: string;
  /** true: a fresh proof can succeed; false: definitive; undefined: a legacy bare 401. */
  retryable?: boolean;
  /** The hub's clock, Unix seconds. */
  serverTime?: number;
}

export async function readAuthRefusal(response: Response): Promise<AuthRefusal> {
  try {
    const body = await response.clone().json() as Record<string, unknown>;
    return {
      reason: typeof body.reason === "string" ? body.reason : undefined,
      retryable: typeof body.retryable === "boolean" ? body.retryable : undefined,
      serverTime: typeof body.server_time === "number" && Number.isFinite(body.server_time) ? body.server_time : undefined,
    };
  } catch {
    return {};
  }
}

/**
 * Which re-pair screen a refusal earns. Only a definitive answer is a lost
 * pairing: an expired credential refreshes, and a legacy bare 401 keeps the
 * old reading (expired by date, else revoked) (cas-d636).
 */
export function authFailureKind(status: number, refusal: AuthRefusal, expiresAt: string, now = Date.now()): AuthFailureKind {
  if (status === 403) return "scope-mismatch";
  if (refusal.reason === "expired") return "expired";
  if (refusal.reason === undefined && Date.parse(expiresAt) <= now) return "expired";
  return "revoked";
}

function authFailureMessage(kind: AuthFailureKind, refusal: AuthRefusal): string {
  if (kind === "expired") return "pairing expired";
  if (kind === "scope-mismatch") return "credential ceiling does not grant this operation";
  if (refusal.reason === "key_mismatch") return "this browser's key no longer matches the pairing";
  if (refusal.reason === "idle") return "pairing unused for too long";
  if (refusal.reason === "origin_mismatch") return "pairing belongs to another Cassy Cloud";
  return "pairing was revoked";
}

/**
 * A browser that lacks an API this build needs cannot be fixed by trying
 * again, so it is failed once with the reason on screen instead of retried
 * forever behind a "Connecting…" spinner (report cas-b652, defect D3).
 * Deliberately not inferred from TypeError: fetch rejects with TypeError on an
 * ordinary network failure, which must keep retrying.
 */
export class UnsupportedBrowserError extends Error {}

/**
 * fetch rejects with TypeError when the network fails, and an abort or a
 * timeout rejects with DOMException: neither is an answer from the hub, so
 * neither may be read as a refusal or a protocol verdict (cas-0978). Fatal is
 * never inferred from this; it is declared with UnsupportedBrowserError.
 */
function isNetworkFailure(error: unknown): boolean {
  return error instanceof TypeError || error instanceof DOMException;
}

function unsupportedBrowserReason(): string | undefined {
  return unsupportedBrowserNotice(browserSupport(undefined, "transport"));
}

export class HubConnectionSupervisor {
  private desired = false;
  private attempt = 0;
  private eventAbort?: AbortController;
  private catalogRequest?: Promise<HubSession[]>;
  /** SSE and multiplexed events share this lane; event delivery never waits. */
  private readonly eventCatalog = new CoalescedRefresh(async () => {
    if (!this.desired || this.lifecycle.phase !== "live") return;
    const stream = this.eventAbort;
    try {
      await this.refreshSessions(anySignal([stream?.signal ?? new AbortController().signal, AbortSignal.timeout(SOCKET_PROBE_TIMEOUT_MS)]));
    } catch (error) {
      // cas-eefe: event delivery never waits on this read, and every
      // heartbeat reads the catalog too, so a failed or timed-out read is the
      // heartbeat's evidence to count. Aborting the stream here skipped the
      // four-missed-heartbeats rule: a heartbeat GET this read shares (the
      // catalog flight is coalesced) took a half-open machine from Unsteady
      // straight to Reconnecting. Only a refused pairing ends the stream now.
      if (!(error instanceof AuthenticationError)) return;
      // A cancelled or failed old stream must not abort its replacement.
      if (stream === this.eventAbort && !stream?.signal.aborted) stream?.abort(error);
    }
  }, () => {});
  private retryTimer?: number;
  private heartbeatTimer?: number;
  private missedHeartbeats = 0;
  private lastHeartbeatAt?: number;
  private expiredRefreshAttempted = false;
  /**
   * The hub's clock minus this device's, from its last 401 (cas-d636). A
   * phone whose clock drifted past the hub's proof window signs every proof
   * stale; proofs are signed on the hub's clock instead.
   */
  private clockOffsetMs = 0;
  private resumeStage: ConnectionStage = "resolving";
  private lifecycle: ConnectionSnapshot = {
    phase: "idle", stage: "idle", since: Date.now(), attempt: 0, missedHeartbeats: 0, degraded: false,
  };
  private readonly sockets = new Map<string, WebSocket>();
  private readonly keyframeRequests = new Set<string>();
  private readonly attachLifecycles = new Map<string, AttachSnapshot>();
  private readonly socketAttempts = new Map<string, number>();
  /**
   * Retryable `upstream_unavailable` refusals per session since its last
   * acknowledged send (cas-a355), or since it last stayed live without one
   * (cas-2036). Each one lengthens the next reattach: 1, 2, 4, then 8 s,
   * instead of a fresh 1 s after every live attach.
   */
  private readonly upstreamRefusalStreak = new Map<string, number>();
  /** Pending "stayed live" resets of the refusal streak, per session (cas-2036). */
  private readonly upstreamStreakResets = new Map<string, number>();
  /** Supervisor messages written to each legacy session socket, in order (cas-a355). */
  private readonly legacySends = new WeakMap<WebSocket, string[]>();
  private readonly attachRetryTimers = new Map<string, number>();
  private readonly attachTimeouts = new Map<string, { open?: number; ready?: number }>();
  private readonly timedOutSockets = new WeakSet<WebSocket>();
  private readonly readySockets = new WeakSet<WebSocket>();
  private readonly legacyReadAt = new WeakMap<WebSocket, number>();
  private machineReadAt?: number;
  private machineSocket?: WebSocket;
  private machineSocketReady = false;
  private machineSocketOpening?: Promise<boolean>;
  private machineMultiplex = false;
  private machineProtocolBlocked = false;
  /**
   * Bumped whenever the machine sockets are abandoned (cas-7b31). An opening
   * that was waiting on its ticket when that happened must not go on to open
   * a socket of its own beside the replacement's.
   */
  private machineSocketGeneration = 0;
  private readonly desiredSessions = new Set<string>();
  private readonly machineSubscriptions = new Set<string>();
  private readonly sessionPanes = new Map<string, PaneInfo[]>();
  private healthPing?: { id: number; startedAt: number };
  private readonly eventRecovery = new EventRecovery();
  private readonly diagnostics = new ConnectionDiagnostics();
  private connectionGeneration = 0;
  /**
   * The connection was lost (heartbeats failed, the network went offline, a
   * reconnect failed) since it was last live. Sockets from before the loss may
   * be half-open: open to the browser, dead on the wire. They are replaced
   * once the machine answers again rather than trusted (cas-0978).
   */
  private connectionLost = false;
  /** Settles a machine socket still opening when it is abandoned. */
  private abandonMachineSocketOpening?: () => void;
  private removeNetworkListeners?: () => void;
  private probeTimer?: number;
  /** The id of a probe's health ping still unanswered; separate from the
   * heartbeat's, which may be sent (and replaced) while a probe waits. */
  private probePingId?: number;
  private hiddenAt?: number;

  constructor(readonly machine: StoredMachine, private readonly callbacks: HubCallbacks) {}

  start(): void {
    if (this.desired) return;
    this.desired = true;
    this.listenForNetworkChanges();
    void this.connect();
  }

  stop(): void {
    this.desired = false;
    this.removeNetworkListeners?.();
    this.removeNetworkListeners = undefined;
    if (this.probeTimer !== undefined) window.clearTimeout(this.probeTimer);
    this.probeTimer = undefined;
    this.eventAbort?.abort();
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    if (this.heartbeatTimer !== undefined) window.clearInterval(this.heartbeatTimer);
    this.heartbeatTimer = undefined;
    this.clearAttachRetries();
    this.clearAttachTimeouts();
    for (const session of [...this.upstreamStreakResets.keys()]) this.cancelUpstreamStreakReset(session);
    this.machineSocket?.close(1000, "machine removed");
    this.machineSocket = undefined;
    this.machineSocketReady = false;
    this.machineSocketOpening = undefined;
    this.desiredSessions.clear();
    this.machineSubscriptions.clear();
    this.sessionPanes.clear();
    this.healthPing = undefined;
    for (const socket of this.sockets.values()) socket.close(1000, "machine removed");
    this.sockets.clear();
    for (const session of this.attachLifecycles.keys()) {
      this.transitionAttach(session, "idle", "idle");
      this.attachLifecycles.delete(session);
    }
    this.transition("idle", "idle");
  }

  snapshot(): ConnectionSnapshot { return this.lifecycle; }

  attachSnapshot(session: string): AttachSnapshot | undefined { return this.attachLifecycles.get(session); }

  attachSnapshots(): ReadonlyMap<string, AttachSnapshot> { return this.attachLifecycles; }

  retry(): void {
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.eventAbort?.abort();
    this.resumeStage = this.lifecycle.stage === "idle" || this.lifecycle.stage === "live"
      ? "resolving" : this.lifecycle.stage;
    this.attempt = 0;
    this.desired = true;
    void this.connect();
  }

  private transition(phase: ConnectionPhase, stage: ConnectionStage, update: Partial<ConnectionSnapshot> = {}): void {
    const now = Date.now();
    this.lifecycle = {
      phase,
      stage,
      since: now,
      connectingSince: connectingAnchor(this.lifecycle, phase, now),
      attempt: this.attempt,
      missedHeartbeats: this.missedHeartbeats,
      degraded: this.missedHeartbeats >= DEGRADED_AFTER_MISSED_HEARTBEATS,
      lastSuccessAt: this.lastHeartbeatAt || this.lifecycle.lastSuccessAt,
      nextRetryAt: update.retryInMs === undefined ? undefined : now + update.retryInMs,
      cause: phase === "live" || phase === "idle" ? undefined : this.lifecycle.cause,
      networkAccessHelp: phase === "live" || phase === "idle" || update.authFailure ? undefined : this.lifecycle.networkAccessHelp,
      ...update,
    };
    this.diagnostics.record(this.lifecycle, this.connectionGeneration);
    this.callbacks.onState(this.lifecycle);
  }

  private transitionAttach(session: string, phase: ConnectionPhase, stage: ConnectionStage, update: Partial<AttachSnapshot> = {}): void {
    const now = Date.now();
    const prior = this.attachLifecycles.get(session);
    const attachSince = phase === "live" || phase === "idle"
      ? undefined
      : prior && prior.phase !== "live" && prior.phase !== "idle"
        ? (prior.attachSince ?? prior.since)
        : now;
    const snapshot: AttachSnapshot = {
      session,
      phase,
      stage,
      since: now,
      attachSince,
      attempt: this.socketAttempts.get(session) ?? 0,
      missedHeartbeats: 0,
      degraded: false,
      lastSuccessAt: phase === "live" ? now : prior?.lastSuccessAt,
      nextRetryAt: update.retryInMs === undefined ? undefined : now + update.retryInMs,
      cause: update.sessionOnly ? { code: "session_upstream_unavailable", layer: "session", retryable: true }
        : phase === "live" || phase === "idle" ? undefined : prior?.cause,
      // A session-only drop stays one through its retry; another failure, or
      // being live again, ends it (cas-d15c).
      sessionOnly: phase === "failed" || phase === "live" || phase === "idle" ? undefined : prior?.sessionOnly,
      networkAccessHelp: phase === "live" || phase === "idle" || update.authFailure ? undefined : prior?.networkAccessHelp,
      ...update,
    };
    this.attachLifecycles.set(session, snapshot);
    this.diagnostics.record(snapshot, this.machineSocketGeneration, session);
    this.callbacks.onAttachState?.(session, snapshot);
  }

  private async connect(): Promise<void> {
    if (!this.desired) return;
    this.connectionGeneration += 1;
    let stage = this.resumeStage;
    let liveSince: number | undefined;
    try {
      const unsupported = unsupportedBrowserReason();
      if (unsupported) throw new UnsupportedBrowserError(unsupported);
      if (stage === "resolving") {
        this.transition("resolving", "resolving");
        await this.withStageTimeout("resolving", async () => { new URL(this.machine.baseUrl); });
        stage = "dialing";
      }
      if (stage === "dialing") {
        this.transition("dialing", "dialing");
        await this.withStageTimeout("dialing", (signal) => this.probeHealth(signal));
        stage = "auth";
      }
      if (stage === "auth") {
        this.transition("auth", "auth");
        await this.withStageTimeout("auth", async (signal) => {
          await this.refreshMachineInfo(signal);
          await this.refreshSessions(signal);
        });
        stage = "attaching";
      }
      this.transition("attaching", "attaching");
      const response = await this.withStageTimeout("attaching", (signal) => this.openEventStream(signal));
      liveSince = Date.now();
      this.resumeStage = "resolving";
      this.missedHeartbeats = 0;
      this.lastHeartbeatAt = Date.now();
      const recovering = this.connectionLost;
      this.connectionLost = false;
      this.transition("live", "live");
      this.startHeartbeat();
      // Event-stream loss alone does not condemn terminal transports that
      // still answer. A proved machine outage already abandoned them.
      if (recovering) this.reattachDesired("Reconnected after the network changed");
      // A session can become ready before this event stream. Its earlier
      // flush was fenced while the machine was attaching; wake it now, after
      // missing or stale recovery sockets have been replaced.
      this.releaseHeldMessages();
      await this.consumeEvents(response, this.eventAbort!.signal);
      if (this.desired) throw new Error("hub event stream closed");
    } catch (error) {
      if (!this.desired) return;
      // Headers alone do not prove recovery. A flapping event stream must
      // survive the settling window before its retry streak starts over.
      if (liveSince !== undefined && Date.now() - liveSince >= EVENT_STREAM_STABLE_MS) this.attempt = 0;
      if (error instanceof UnsupportedBrowserError) {
        this.stopHeartbeat();
        this.transition("failed", stage, { reason: error.message, fatal: true });
        return;
      }
      if (error instanceof DOMException && error.name === "AbortError") {
        if (this.missedHeartbeats < RECONNECT_AFTER_MISSED_HEARTBEATS) return;
        error = new Error(`${this.missedHeartbeats} consecutive heartbeats missed`);
        stage = "dialing";
      }
      if (error instanceof AuthenticationError) {
        let authError = error;
        if (error.kind === "expired" && !this.expiredRefreshAttempted) {
          this.expiredRefreshAttempted = true;
          try {
            await this.refreshCredential();
            this.resumeStage = "auth";
            void this.connect();
            return;
          } catch (refreshError) {
            if (refreshError instanceof AuthenticationError) authError = refreshError;
            else {
              // The refresh may have reached the hub. A transport timeout is
              // not an explicit expiry/revocation; retry without destroying access.
              const delay = Math.min(MACHINE_RETRY_CEILING_MS, backoffDelay(this.attempt++));
              this.transition("backoff", "auth", { cause: this.failureCause(refreshError), retryInMs: delay });
              this.retryTimer = window.setTimeout(() => { this.expiredRefreshAttempted = false; void this.connect(); }, delay);
              return;
            }
          }
        }
        this.blockAuthentication(authError.kind, authError.message);
        return;
      }
      // A fetch rejection does not say whether the pairing is valid. Chrome
      // can block authenticated requests before they reach the hub (preflight
      // or local-network permission), even when a health probe succeeds.
      // Only AuthenticationError above is an explicit refusal (cas-b85a).
      this.stopHeartbeat();
      this.connectionLost = true;
      this.resumeStage = stage;
      const reason = error instanceof Error ? error.message : "unknown connection failure";
      const target = new URL(this.machine.baseUrl).host;
      const cause = this.failureCause(error);
      this.transition("failed", stage, { reason: stageFailureDetail(stage, target, reason), cause });
      // Capped well below the backoff's 30 s ceiling: a network that returns
      // without an event (Tailscale switched on) is noticed within 10 s.
      const delay = Math.min(MACHINE_RETRY_CEILING_MS, backoffDelay(this.attempt++));
      const networkAccessHelp = isNetworkFailure(error) ? await localNetworkAccessHelp(this.machine.baseUrl, this.machine.label, permission => { cause.permission = permission; }) : undefined;
      if (!this.desired) return;
      this.transition("backoff", stage, { reason: stageFailureDetail(stage, target, reason), retryInMs: delay, networkAccessHelp, cause });
      this.retryTimer = window.setTimeout(() => {
        this.retryTimer = undefined;
        if (this.desired) void this.connect();
      }, delay);
    }
  }

  private async withStageTimeout<T>(stage: Exclude<ConnectionStage, "idle" | "live">, task: (signal: AbortSignal) => Promise<T>): Promise<T> {
    return withRequestDeadline(task, undefined, STAGE_TIMEOUT_MS[stage]);
  }

  private failureCause(error: unknown): CauseEvidence {
    if (error instanceof EventRecoveryError) return { code: error.code, layer: "events", retryable: true, requestId: error.requestId };
    if (error instanceof AuthenticationError) return { code: `auth_${error.kind.replaceAll("-", "_")}` as CauseEvidence["code"], layer: "auth", retryable: false, requestId: error.requestId };
    if (error instanceof TransientAuthError) return { code: "proof_refused", layer: "auth", retryable: true };
    if (error instanceof DOMException && error.name === "TimeoutError") return { code: "request_timeout", layer: "http", retryable: true };
    if (error instanceof HubRequestError) return { code: error.code === "health_probe" && error.status === 503 ? "health_http_unavailable" : "http_refused", layer: "http", retryable: true, status: error.status, requestId: error.requestId };
    if (typeof navigator !== "undefined" && navigator.onLine === false) return { code: "browser_offline", layer: "browser", retryable: true };
    if (isNetworkFailure(error)) return { code: "network_or_browser_policy_unknown", layer: "browser", retryable: true, permission: "unknown" };
    if (error instanceof UnsupportedBrowserError) return { code: "unsupported_browser", layer: "browser", retryable: false };
    return { code: "stream_closed", layer: "events", retryable: true };
  }

  private async probeHealth(signal: AbortSignal): Promise<void> {
    const response = await fetch(new URL("/v1/health", this.machine.baseUrl), { signal, cache: "no-store", credentials: "omit" });
    if (!response.ok) throw new HubRequestError(`daemon health failed (${response.status})`, response.status, "health_probe");
  }

  /**
   * An authenticated fetch (cas-d636). A 401 that is not definitive (a
   * retryable reason, or a legacy bare 401) is tried once more with a fresh
   * proof on the hub's clock: the common case is a proof signed before the
   * phone slept and sent when it woke. A second retryable refusal throws
   * TransientAuthError, retried like a network failure. Any other 401 or 403
   * is returned for the caller to read as a lost pairing.
   */
  private installationMutation = false;

  private async adoptInstallation(): Promise<boolean> {
    if (this.machine.credentialGeneration === undefined && typeof indexedDB === "undefined") return false;
    const pending = (await installationStore.list()).some((r) => r.pending && r.id.split("@")[0] === this.machine.id);
    if (pending) throw new Error("Installation cleanup is pending. Retry cleanup before reconnecting.");
    const installed = (await catalog.snapshot()).machines.find((m) => m.id === this.machine.id && m.baseUrl === this.machine.baseUrl);
    if (!installed || installed.credentialId === this.machine.credentialId) return false;
    if ((installed.credentialGeneration ?? 0) <= (this.machine.credentialGeneration ?? 0)) return false;
    Object.assign(this.machine, installed);
    this.expiredRefreshAttempted = false;
    return true;
  }

  private async authorizedFetch(method: string, path: string, init: RequestInit = {}): Promise<{ response: Response; refusal: AuthRefusal }> {
    // The proof binds the bare path; the hub rejects an htu with a query.
    const htu = path.split("?")[0] ?? path;
    const send = async () => {
      const proof = await dpopHeaders(this.machine, method, htu, Date.now() + this.clockOffsetMs);
      init.signal?.throwIfAborted();
      return fetch(new URL(path, this.machine.baseUrl), {
      ...init,
      method,
      headers: { ...(init.headers as Record<string, string> | undefined), ...proof },
      cache: "no-store",
      credentials: "omit",
    });
    };
    let response = await send();
    if ((response.status === 401 || response.status === 403) && (this.machine.credentialGeneration !== undefined || typeof indexedDB !== "undefined") && !this.installationMutation) {
      // A peer may have rotated between signing and arrival. Wait for its durable
      // commit/rollback, then retry only a different accepted credential.
      if (await installationLock(this.machine.id, () => this.adoptInstallation(), init.signal ?? undefined)) response = await send();
    }
    if (response.status !== 401) return { response, refusal: {} };
    let refusal = await readAuthRefusal(response);
    if (refusal.retryable === false) return { response, refusal };
    this.adoptHubClock(refusal.serverTime);
    response = await send();
    if (response.status !== 401) return { response, refusal: {} };
    refusal = await readAuthRefusal(response);
    if (refusal.retryable === true) throw new TransientAuthError(refusal.reason ?? "invalid_proof");
    return { response, refusal };
  }

  private adoptHubClock(serverTime: number | undefined): void {
    if (serverTime === undefined) return;
    const offset = serverTime * 1000 - Date.now();
    // Inside a few seconds is latency and rounding, not drift.
    this.clockOffsetMs = Math.abs(offset) > 5_000 ? offset : 0;
  }

  async request<T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    return withRequestDeadline(async boundedSignal => {
    const startedAt = performance.now();
    const { response, refusal } = await this.authorizedFetch(method, path, {
      headers: body === undefined ? {} : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: boundedSignal,
    });
    if (response.status === 401 || response.status === 403) {
      const kind = authFailureKind(response.status, refusal, this.machine.expiresAt);
      throw new AuthenticationError(kind, authFailureMessage(kind, refusal), response.headers?.get("x-cas-request-id") ?? undefined);
    }
    if (!response.ok) throw await hubRequestError(method, path, response);
    this.callbacks.onLatency?.(Math.max(0, Math.round(performance.now() - startedAt)));
    if (response.status === 204) return undefined as T;
    const result = await response.json() as T;
    boundedSignal.throwIfAborted();
    return result;
    }, signal);
  }

  async refreshSessions(signal: AbortSignal = AbortSignal.timeout(SOCKET_PROBE_TIMEOUT_MS)): Promise<HubSession[]> {
    signal.throwIfAborted();
    if (!this.catalogRequest) this.catalogRequest = (async () => {
      const response = await this.request<{ sessions: HubSession[]; freshness_threshold_secs?: number }>("GET", sessionsPath(revealDormant()), undefined, signal);
      signal.throwIfAborted();
      this.callbacks.onSessions(response.sessions, response.freshness_threshold_secs);
      return response.sessions;
    })().finally(() => { this.catalogRequest = undefined; });
    const request = this.catalogRequest;
    // A caller can be cancelled before the wrapper's task runs. Keep a
    // rejection observer on the shared flight, and retain its identity even
    // after its finally clears catalogRequest.
    void request.catch(() => {});
    return withRequestDeadline(() => request, signal, SOCKET_PROBE_TIMEOUT_MS);
  }

  private async refreshMachineInfo(signal?: AbortSignal): Promise<void> {
    try {
      const info = await this.request<HubMachineInfo>("GET", "/v1/machine", undefined, signal);
      if (typeof info.version === "string") this.diagnostics.build(info.version);
      this.machineMultiplex = info.capabilities.includes("machine_multiplex_v2");
      this.callbacks.onMachineInfo?.(info);
    } catch (error) {
      if (error instanceof AuthenticationError || error instanceof TransientAuthError) throw error;
      // Older hubs can still offer the read-only session surface. The UI shows
      // a visible compatibility warning and leaves capability-gated controls off.
      this.callbacks.onMachineInfo?.(undefined);
    }
  }

  /**
   * A short-lived signed URL for viewing an artifact the session published
   * (cassy#910). Unlike `request`, a refusal is an answer, not an exception:
   * the machine's stable error code tells Commander what to say.
   */
  async artifactView(session: string, artifactId: string): Promise<ArtifactViewResult> {
    return withRequestDeadline(async signal => {
    const path = `/v1/sessions/${encodeURIComponent(session)}/artifacts/${encodeURIComponent(artifactId)}/url`;
    const { response } = await this.authorizedFetch("GET", path, { signal });
    const body = await response.json().catch(() => undefined) as Record<string, unknown> | undefined;
    if (response.ok && body && typeof body.url === "string") {
      return { ok: true, view: body as unknown as ArtifactView };
    }
    return {
      ok: false,
      status: response.status,
      code: typeof body?.error === "string" ? body.error : undefined,
      detail: typeof body?.status === "string" ? body.status : null,
    };
    });
  }

  /** The machine's main project folders and launch roots (cas-41b9). */
  async projects(signal?: AbortSignal): Promise<ProjectCatalog> {
    return this.request("GET", "/v1/projects", undefined, signal);
  }

  /** Each launch CLI's account profiles on the machine (cas-7b52). */
  async launchProfiles(signal?: AbortSignal): Promise<LaunchProfiles> {
    return this.request("GET", "/v1/launch/profiles", undefined, signal);
  }

  /** One folder under a configured launch root. */
  async browseProjects(root: string, path: string, signal?: AbortSignal): Promise<BrowseListing> {
    const query = new URLSearchParams({ root, path });
    return this.request("GET", `/v1/projects/browse?${query}`, undefined, signal);
  }

  /**
   * Start (or join) a factory session (cas-4c5a). A refusal is an answer, not
   * an exception: its code says what the operator has to fix, and a scope
   * refusal is not a lost pairing.
   */
  async launchSession(request: LaunchRequest): Promise<LaunchResult> {
    return withRequestDeadline(async signal => {
    const { response } = await this.authorizedFetch("POST", "/v1/sessions", {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(request),
      signal,
    });
    const body = await response.json().catch(() => undefined) as Record<string, unknown> | undefined;
    if (response.ok && body && typeof body.session === "string") {
      return { ok: true, session: body.session, attached: body.attached === true };
    }
    return {
      ok: false,
      status: response.status,
      ...(typeof body?.error === "string" ? { code: body.error } : {}),
      ...(typeof body?.detail === "string" ? { detail: body.detail } : typeof body?.reason === "string" ? { detail: body.reason } : {}),
    };
    }, undefined, 30_000);
  }

  /**
   * The one-time "Allow managing workers" grant (cas-d382): add factory-operate
   * to this paired device, as session launch is added. The hub allows it only
   * to a device that already holds the control scopes; a 403 says the pairing
   * itself must change.
   */
  async enableFactoryOperate(): Promise<void> {
    return withRequestDeadline(async signal => {
    const { response } = await this.authorizedFetch("POST", "/v1/auth/scopes", {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ add: ["factory-operate"] }),
      signal,
    });
    if (!response.ok) {
      const error = await hubRequestError("POST", "/v1/auth/scopes", response);
      throw new ScopeGrantError(response.status, response.status === 403
        ? "This pairing can't allow managing workers. Pair with a control invitation, then try again."
        : `Could not allow managing workers (${response.status}). Try again.`, error.code, error.detail);
    }
    const body = await response.json() as { scopes: StoredMachine["scopes"] };
    signal.throwIfAborted();
    this.machine.scopes = body.scopes;
    await this.callbacks.onCredentialRefreshed?.(this.machine);
    });
  }

  /** Add session launch to this paired device; keep the live and stored scope sets in sync. */
  async enableSessionLaunch(): Promise<void> {
    return withRequestDeadline(async signal => {
    const { response } = await this.authorizedFetch("POST", "/v1/auth/scopes", {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ add: ["session-launch"] }),
      signal,
    });
    if (!response.ok) throw new SessionLaunchGrantError(response.status, response.status === 403
      ? "This pairing no longer has control access. Pair with a control invitation, then try again."
      : `Could not enable session launch (${response.status}). Try again.`);
    const body = await response.json() as { scopes: StoredMachine["scopes"] };
    signal.throwIfAborted();
    this.machine.scopes = body.scopes;
    await this.callbacks.onCredentialRefreshed?.(this.machine);
    });
  }

  /**
   * Run one structured fleet operation (cas-a474, fleet-operations brief):
   * the hub dedupes `op_id`, refuses a stale `expected` with 409
   * {error:"stale", current}, and announces FleetChanged. No terminal lease.
   */
  async operation(session: string, body: { op_id: string; op: Record<string, unknown>; expected: Record<string, unknown> }): Promise<{ op_id: string; outcome?: Record<string, unknown> }> {
    return this.request("POST", `/v1/sessions/${encodeURIComponent(session)}/operations`, body);
  }

  async status(session: string): Promise<Record<string, unknown>> {
    return this.request("GET", `/v1/sessions/${encodeURIComponent(session)}/status`);
  }

  async lease(session: string): Promise<LeaseState> {
    return this.request("GET", `/v1/sessions/${encodeURIComponent(session)}/lease`);
  }

  async acquireLease(session: string, force = false): Promise<LeaseState> {
    return this.request("POST", `/v1/sessions/${encodeURIComponent(session)}/lease`, { force });
  }

  /** Escalate this observer session up to its paired grant ceiling. */
  async requestControl(session: string, force = false): Promise<LeaseState> {
    return this.acquireLease(session, force);
  }

  /**
   * End a session (cas-55a4): the hub stops its daemon the way `cas kill`
   * does. Needs factory-manage; a refusal throws like any other request.
   */
  async endSession(session: string): Promise<{ session: string; outcome: "ended" | "cleaned_stale" }> {
    return this.request("DELETE", `/v1/sessions/${encodeURIComponent(session)}`);
  }

  async releaseLease(session: string): Promise<void> {
    await this.request("DELETE", `/v1/sessions/${encodeURIComponent(session)}/lease`);
  }

  async diagnose(): Promise<Record<string, unknown>> {
    let report: unknown;
    try { report = await this.request<Record<string, unknown>>("GET", "/v1/diagnostics"); }
    catch (error) { this.diagnostics.record({ ...this.lifecycle, since: Date.now(), cause: this.failureCause(error) }, this.connectionGeneration); }
    return this.diagnostics.export(report, typeof navigator === "undefined" ? undefined : navigator.onLine);
  }

  private async openEventStream(signal: AbortSignal): Promise<Response> {
    this.eventAbort = new AbortController();
    const path = "/v1/events";
    const { response, refusal } = await this.authorizedFetch("GET", path, {
      // AbortSignal.any is Chrome 116+; calling it bare took the whole event
      // stream out on older engines (cas-b652 D3).
      signal: anySignal([this.eventAbort.signal, signal]),
    });
    if (response.status === 401 || response.status === 403) {
      const kind = authFailureKind(response.status, refusal, this.machine.expiresAt);
      throw new AuthenticationError(kind, `event-stream authentication failed: ${authFailureMessage(kind, refusal)}`);
    }
    if (!response.ok || !response.body) throw new Error(`event stream failed (${response.status})`);
    return response;
  }

  private async consumeEvents(response: Response, signal: AbortSignal): Promise<void> {
    if (!response.body) throw new Error("event stream closed before attach");
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    let buffer = "";
    let replaying = false;
    try {
      for (;;) {
        signal.throwIfAborted();
        const { value, done } = await withRequestDeadline(() => reader.read(), signal, 60_000);
        if (done) return;
        buffer += value;
        buffer = buffer.replaceAll("\r\n", "\n");
        let boundary = buffer.indexOf("\n\n");
        while (boundary >= 0) {
          if (boundary > 131_072) throw new Error("event frame exceeded limit");
          const block = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          const data = block.split("\n").filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trim()).join("\n");
          if (data) {
            const event = JSON.parse(data) as Record<string, unknown>;
            if (event.kind === "stream_metadata" && typeof event.epoch === "string") {
              replaying = true;
              const reason = this.eventRecovery.begin(event.epoch, Number(event.oldest_sequence), Number(event.latest_sequence));
              if (reason) this.recordEventRecovery(reason === "epoch_changed" ? "event_epoch_changed" : "event_retention_gap");
            } else if (event.kind === "replay_complete") { replaying = false; }
            else if (event.kind === "viewer_lagged") {
              const requestId = typeof event.request_id === "string" ? event.request_id : undefined;
              this.recordEventRecovery("viewer_lagged", requestId);
              throw new EventRecoveryError("viewer_lagged", requestId);
            } else this.deliverMachineEvent(event, replaying);
            void this.eventCatalog.request();
          }
          boundary = buffer.indexOf("\n\n");
        }
        if (buffer.length > 131_072) throw new Error("event frame exceeded limit");
      }
    } finally {
      await withRequestDeadline(() => reader.cancel(), undefined, 1_000).catch(() => {});
      reader.releaseLock();
    }
  }

  private startHeartbeat(): void {
    this.stopHeartbeat();
    this.heartbeatTimer = window.setInterval(() => void this.heartbeat(), HEARTBEAT_INTERVAL_MS);
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer !== undefined) window.clearInterval(this.heartbeatTimer);
    this.heartbeatTimer = undefined;
  }

  private async heartbeat(): Promise<void> {
    // cas-7b31 (cas-05c0 QA): a stopped or refused connection has no
    // heartbeat. One that ran on after a pairing refusal turned the machine
    // "live" again on its next beat, which erased "Needs pairing" from the
    // controls and the rail while the header still said it.
    if (!this.desired || this.lifecycle.phase !== "live") return;
    const started = performance.now();
    try {
      await this.refreshSessions(AbortSignal.timeout(3_000));
      if (this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN) {
        if (this.healthPing) {
          this.missedHeartbeats += 1;
          this.transition("live", "live", { reason: "machine WebSocket heartbeat missed" });
          if (this.missedHeartbeats >= RECONNECT_AFTER_MISSED_HEARTBEATS) {
            // The hub answers HTTP but not this socket: it is half-open. A
            // close() on it can wait out a closing handshake that never
            // comes, so it is dropped and replaced now (cas-0978).
            this.missedHeartbeats = 0;
            this.reattachDesired("Machine terminal transport stopped answering", true);
            return;
          }
        }
        const id = Date.now();
        this.healthPing = { id, startedAt: started };
        this.machineSocket.send(JSON.stringify({ channel: "health", ping: id }));
        return;
      }
      await this.request("GET", "/v1/machine", undefined, AbortSignal.timeout(3_000));
      const wasUnsteady = this.unsteady();
      this.missedHeartbeats = 0;
      this.lastHeartbeatAt = Date.now();
      this.transition("live", "live", { latencyMs: Math.round(performance.now() - started) });
      if (wasUnsteady) this.releaseHeldMessages();
    } catch (error) {
      // Stopped or refused while this beat was in flight: not a live machine.
      if (!this.desired || this.lifecycle.phase !== "live") return;
      this.missedHeartbeats += 1;
      this.transition("live", "live", { reason: error instanceof Error ? error.message : "heartbeat failed", cause: this.failureCause(error) });
      if (this.missedHeartbeats >= RECONNECT_AFTER_MISSED_HEARTBEATS) this.connectionLostNow("Lost connection to the machine");
    }
  }

  /**
   * The machine stopped answering (heartbeats failed, or the browser went
   * offline): stop trusting its sockets and reconnect from the dialing stage.
   */
  private connectionLostNow(reason: string): void {
    this.connectionLost = true;
    this.missedHeartbeats = Math.max(this.missedHeartbeats, RECONNECT_AFTER_MISSED_HEARTBEATS);
    this.stopHeartbeat();
    this.abandonSockets(reason);
    this.resumeStage = "dialing";
    this.eventAbort?.abort();
  }

  /**
   * Drop every terminal socket without waiting for it to close. A half-open
   * socket never finishes a closing handshake, so its onclose may not fire
   * for minutes; its handlers are detached and the state it owned is reset
   * here instead.
   */
  private abandonSockets(reason: string): void {
    this.machineSocketGeneration += 1;
    const machineSocket = this.machineSocket;
    if (machineSocket) {
      machineSocket.onopen = null; machineSocket.onmessage = null; machineSocket.onerror = null; machineSocket.onclose = null;
      try { machineSocket.close(4000, "abandoned"); } catch { /* already closing */ }
    }
    this.abandonMachineSocketOpening?.();
    this.abandonMachineSocketOpening = undefined;
    this.machineSocket = undefined;
    this.machineSocketReady = false;
    this.machineSocketOpening = undefined;
    this.machineSubscriptions.clear();
    this.healthPing = undefined;
    this.probePingId = undefined;
    if (this.probeTimer !== undefined) window.clearTimeout(this.probeTimer);
    this.probeTimer = undefined;
    for (const socket of this.sockets.values()) {
      socket.onopen = null; socket.onmessage = null; socket.onerror = null; socket.onclose = null;
      try { socket.close(4000, "abandoned"); } catch { /* already closing */ }
    }
    this.sockets.clear();
    this.keyframeRequests.clear();
    this.clearAttachTimeouts();
    for (const session of this.desiredSessions) {
      const retry = this.attachRetryTimers.get(session);
      if (retry !== undefined) window.clearTimeout(retry);
      this.attachRetryTimers.delete(session);
      this.transitionAttach(session, "failed", "dialing", { reason });
    }
  }

  /** Recover event delivery without discarding recently speaking terminals. */
  private reattachDesired(reason: string, force = false): void {
    const now = Date.now();
    const machineHealthy = this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN
      && this.machineReadAt !== undefined && now - this.machineReadAt < ATTACH_LIVENESS_MS;
    if (force || (this.machineSocket && !machineHealthy)) {
      this.abandonSockets(reason);
      this.socketAttempts.clear();
    } else {
      for (const [session, socket] of this.sockets) {
        const lastRead = this.legacyReadAt.get(socket);
        // Opening sockets already have bounded open/Welcome deadlines. Let
        // those deadlines judge them rather than restart each opening.
        const opening = socket.readyState === WebSocket.CONNECTING
          || (socket.readyState === WebSocket.OPEN && !this.readySockets.has(socket));
        if (opening || (socket.readyState === WebSocket.OPEN && lastRead !== undefined
          && now - lastRead < ATTACH_LIVENESS_MS)) continue;
        socket.onopen = null; socket.onmessage = null; socket.onerror = null; socket.onclose = null;
        try { socket.close(4000, "abandoned"); } catch { /* already closing */ }
        this.sockets.delete(session);
        this.clearAttachTimeouts(session);
        for (const key of this.keyframeRequests) if (key.startsWith(`${session}:`)) this.keyframeRequests.delete(key);
        this.transitionAttach(session, "failed", "dialing", { reason });
      }
    }
    for (const session of this.desiredSessions) {
      if (!this.attachRetryTimers.has(session)) void this.attach(session);
    }
  }

  /**
   * The browser's own hints that the network changed under the page: back
   * online, a different connection type, woken from sleep or a frozen tab.
   * None of them is proof; each prompts a check now instead of waiting for
   * the next backoff timer or four missed heartbeats (cas-0978).
   */
  private listenForNetworkChanges(): void {
    if (typeof window === "undefined" || typeof window.addEventListener !== "function" || typeof document === "undefined" || this.removeNetworkListeners) return;
    const changed = () => this.networkChanged();
    const offline = () => { if (this.desired && this.lifecycle.phase === "live") this.connectionLostNow("The network went offline"); };
    // A tab switch is not a network change; coming back after long enough to
    // have slept (or had the radio change) is treated as one.
    const visibility = () => {
      if (document.visibilityState === "hidden") { this.hiddenAt = Date.now(); return; }
      const hiddenFor = this.hiddenAt === undefined ? 0 : Date.now() - this.hiddenAt;
      this.hiddenAt = undefined;
      if (hiddenFor >= HEARTBEAT_INTERVAL_MS) this.networkChanged();
    };
    const connection = (navigator as Navigator & { connection?: EventTarget & { type?: string } }).connection;
    let networkType = connection?.type;
    const transportChanged = () => {
      const nextType = connection?.type;
      // change also fires for rtt/downlink/effectiveType estimates under load.
      // Those are not a changed route, and a failed opportunistic probe must
      // not turn one missed heartbeat into four (cas-eefe).
      if (typeof nextType !== "string" || nextType === networkType) return;
      networkType = nextType;
      changed();
    };
    window.addEventListener("online", changed);
    window.addEventListener("offline", offline);
    window.addEventListener("pageshow", changed);
    document.addEventListener("resume", changed);
    document.addEventListener("visibilitychange", visibility);
    connection?.addEventListener?.("change", transportChanged);
    this.removeNetworkListeners = () => {
      window.removeEventListener("online", changed);
      window.removeEventListener("offline", offline);
      window.removeEventListener("pageshow", changed);
      document.removeEventListener("resume", changed);
      document.removeEventListener("visibilitychange", visibility);
      connection?.removeEventListener?.("change", transportChanged);
    };
  }

  private networkChanged(): void {
    if (!this.desired) return;
    const { phase, fatal, authFailure } = this.lifecycle;
    if (phase === "backoff" || (phase === "failed" && !fatal && !authFailure)) {
      // Waiting out a backoff: try again now, from a fresh schedule.
      if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
      this.retryTimer = undefined;
      void this.connect();
      return;
    }
    if (phase === "live") void this.probeNow();
  }

  /**
   * Live on paper: prove it. HTTP must answer within the probe window, and so
   * must the machine socket's health ping; a socket that does not is
   * half-open and is replaced. Recent daemon frames prove a legacy attach
   * still answers, so a hint alone must not replace it.
   */
  private async probeNow(): Promise<void> {
    try {
      await this.request("GET", "/v1/machine", undefined, AbortSignal.timeout(SOCKET_PROBE_TIMEOUT_MS));
    } catch (error) {
      if (error instanceof AuthenticationError || !this.desired) return;
      this.connectionLostNow(error instanceof Error ? error.message : "machine did not answer");
      return;
    }
    if (!this.desired) return;
    const socket = this.machineSocket;
    if (this.machineSocketReady && socket?.readyState === WebSocket.OPEN) {
      // Well above any heartbeat ping id (Date.now()), and still an unsigned
      // integer, as the hub's `ping: Option<u64>` requires.
      const id = Date.now() * 10 + 1;
      this.probePingId = id;
      socket.send(JSON.stringify({ channel: "health", ping: id }));
      if (this.probeTimer !== undefined) window.clearTimeout(this.probeTimer);
      this.probeTimer = window.setTimeout(() => {
        this.probeTimer = undefined;
        if (this.machineSocket === socket && this.probePingId === id) {
          this.probePingId = undefined;
          this.reattachDesired("Machine terminal transport stopped answering", true);
        }
      }, SOCKET_PROBE_TIMEOUT_MS);
      return;
    }
    if (this.sockets.size > 0) this.reattachDesired("Reattaching after the network changed");
  }

  private async refreshCredential(): Promise<void> {
    const rotate = async () => {
      if (await this.adoptInstallation()) return;
      this.installationMutation = true;
      try {
        const refreshed = await this.request<{ credential: string; credential_id: string; credential_generation?: number; expires_at: string; scopes: StoredMachine["scopes"] }>("POST", "/v1/auth/refresh");
        this.machine.credential = refreshed.credential;
        this.machine.credentialId = refreshed.credential_id;
        this.machine.credentialGeneration = refreshed.credential_generation;
        this.machine.expiresAt = refreshed.expires_at;
        this.machine.scopes = refreshed.scopes;
        this.expiredRefreshAttempted = false;
        await this.callbacks.onCredentialRefreshed?.(this.machine);
        notifyInstallation(this.machine.id);
      } finally { this.installationMutation = false; }
    };
    if (this.machine.credentialGeneration === undefined && typeof indexedDB === "undefined") await rotate();
    else await installationLock(this.machine.id, rotate);
  }

  async attach(session: string): Promise<void> {
    try {
      await this.openAttach(session);
    } catch (error) {
      await this.handleAttachFailure(session, error);
    }
  }

  private async openAttach(session: string): Promise<void> {
    if (!this.desired) return;
    const unsupported = unsupportedBrowserReason();
    if (unsupported) throw new UnsupportedBrowserError(unsupported);
    this.desiredSessions.add(session);
    const retryTimer = this.attachRetryTimers.get(session);
    if (retryTimer !== undefined) {
      window.clearTimeout(retryTimer);
      this.attachRetryTimers.delete(session);
    }
    if (this.machineMultiplex && !this.machineProtocolBlocked) {
      const connected = await this.ensureMachineSocket(session);
      if (connected) {
        this.subscribeMachineSession(session);
        return;
      }
    }
    await this.openLegacyAttach(session);
  }

  private async openLegacyAttach(session: string): Promise<void> {
    const existing = this.sockets.get(session);
    if (existing && (existing.readyState === WebSocket.OPEN || existing.readyState === WebSocket.CONNECTING)) return;
    const generation = this.machineSocketGeneration;
    this.transitionAttach(session, "auth", "auth");
    const ticket = await this.request<{ ticket: string }>("POST", "/v1/auth/websocket-ticket", { session });
    if (!this.desired || generation !== this.machineSocketGeneration || !this.desiredSessions.has(session)) return;
    const current = this.sockets.get(session);
    if (current && (current.readyState === WebSocket.OPEN || current.readyState === WebSocket.CONNECTING)) return;
    const endpoint = new URL(`/v1/sessions/${encodeURIComponent(session)}/attach`, this.machine.baseUrl);
    endpoint.protocol = endpoint.protocol === "https:" ? "wss:" : "ws:";
    endpoint.searchParams.set("ticket", ticket.ticket);
    const socket = new WebSocket(endpoint);
    const frameFence = credentialFence(this.machine);
    this.transitionAttach(session, "dialing", "dialing");
    socket.binaryType = "arraybuffer";
    this.sockets.set(session, socket);
    this.startOpenTimeout(session, socket);
    socket.onopen = () => {
      if (this.sockets.get(session) !== socket) return;
      this.clearAttachTimeout(session, "open");
      this.transitionAttach(session, "attaching", "attaching");
      this.startReadyTimeout(session, socket);
    };
    socket.onmessage = (message) => {
      if (this.sockets.get(session) !== socket) return;
      this.legacyReadAt.set(socket, Date.now());
      void this.handleDaemonMessage(session, message.data, frameFence);
    };
    socket.onclose = (event) => {
      const timedOut = this.timedOutSockets.has(socket);
      const becameReady = this.readySockets.has(socket);
      this.clearAttachTimeouts(session);
      if (this.sockets.get(session) === socket) this.sockets.delete(session);
      if (!this.desired || event.code === 1000) return;
      if (!timedOut) {
        const detail = becameReady ? "Terminal connection closed" : "Terminal connection closed before it became ready";
        this.transitionAttach(session, "failed", becameReady ? "dialing" : (this.attachLifecycles.get(session)?.stage ?? "dialing"), { reason: detail, cause: { code: "socket_closed", layer: "socket", retryable: true, closeCode: event.code } });
        this.callbacks.onSocketError(session, `${detail}. Retrying…`);
      }
      this.scheduleAttach(session);
    };
    socket.onerror = () => {
      const stage = this.attachLifecycles.get(session)?.stage ?? "dialing";
      this.transitionAttach(session, "failed", stage, { reason: "terminal transport error" });
      this.callbacks.onSocketError(session, "terminal transport error");
    };
  }

  private async ensureMachineSocket(session: string): Promise<boolean> {
    if (this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN) return true;
    if (this.machineSocketOpening) return this.machineSocketOpening;
    this.machineSocketOpening = this.openMachineSocket(session).finally(() => {
      this.machineSocketOpening = undefined;
    });
    return this.machineSocketOpening;
  }

  private async openMachineSocket(session: string): Promise<boolean> {
    const generation = this.machineSocketGeneration;
    this.transitionAttach(session, "auth", "auth");
    let ticket: { ticket: string };
    try {
      ticket = await this.request<{ ticket: string }>("POST", "/v1/auth/websocket-ticket", {});
    } catch (error) {
      if (error instanceof AuthenticationError || error instanceof TransientAuthError) throw error;
      // A network failure says nothing about the hub's protocol: without this
      // a ticket request lost to a network switch downgraded the page to
      // per-session sockets for the rest of its life (cas-0978).
      if (isNetworkFailure(error)) throw error;
      // A hub from before machine protocol v2 advertised no capability, but a
      // rolling upgrade can briefly expose stale machine metadata. Preserve
      // the old per-session attach as a bounded compatibility fallback.
      this.machineMultiplex = false;
      return false;
    }
    // Abandoned while the ticket was on its way: the replacement opening owns
    // the machine socket now (cas-7b31).
    if (!this.desired || generation !== this.machineSocketGeneration) return true;
    const endpoint = new URL("/v1/attach", this.machine.baseUrl);
    endpoint.protocol = endpoint.protocol === "https:" ? "wss:" : "ws:";
    endpoint.searchParams.set("ticket", ticket.ticket);
    const socket = new WebSocket(endpoint);
    const frameFence = credentialFence(this.machine);
    socket.binaryType = "arraybuffer";
    this.machineSocket = socket;
    for (const desired of this.desiredSessions) this.transitionAttach(desired, "dialing", "dialing");

    return new Promise<boolean>((resolve) => {
      let settled = false;
      this.abandonMachineSocketOpening = () => {
        if (settled) return;
        settled = true;
        clearTimers();
        resolve(true);
      };
      let openTimer: number | undefined = window.setTimeout(() => {
        if (socket.readyState !== WebSocket.CONNECTING) return;
        settled = true;
        resolve(true);
        for (const desired of this.desiredSessions) {
          this.transitionAttach(desired, "failed", "dialing", { reason: "Stuck dialing machine WebSocket — node may be offline (5s)" });
          this.callbacks.onSocketError(desired, "Stuck dialing machine WebSocket — node may be offline (5s). Retrying…");
          this.scheduleAttach(desired);
        }
        socket.close();
      }, STAGE_TIMEOUT_MS.dialing);
      let handshakeTimer: number | undefined;
      const clearTimers = () => {
        if (openTimer !== undefined) window.clearTimeout(openTimer);
        if (handshakeTimer !== undefined) window.clearTimeout(handshakeTimer);
        openTimer = undefined;
        handshakeTimer = undefined;
      };
      const protocolFailure = (detail: string) => {
        clearTimers();
        // cas-7b31: a socket another opening has replaced says nothing about
        // the hub's protocol. Its handshake timer used to fire after the
        // replacement was ready, mark the protocol blocked and fail the live
        // session, which then stayed "Reconnecting" for good.
        if (this.machineSocket !== socket) {
          if (!settled) {
            settled = true;
            resolve(true);
          }
          try { socket.close(4000, "replaced"); } catch { /* already closing */ }
          return;
        }
        this.machineProtocolBlocked = true;
        for (const desired of this.desiredSessions) {
          this.transitionAttach(desired, "failed", "attaching", { reason: detail });
          this.callbacks.onSocketError(desired, detail);
        }
        if (!settled) {
          settled = true;
          resolve(true);
        }
        // A page may close with 1000 or 3000–4999 only; 1002 threw
        // InvalidAccessError from the handshake timer (cas-7b31).
        socket.close(4002, "protocol mismatch");
      };
      socket.onopen = () => {
        if (this.machineSocket !== socket) return;
        if (openTimer !== undefined) window.clearTimeout(openTimer);
        openTimer = undefined;
        for (const desired of this.desiredSessions) this.transitionAttach(desired, "attaching", "attaching");
        socket.send(JSON.stringify({ proto: 2 }));
        handshakeTimer = window.setTimeout(() => {
          protocolFailure("Machine protocol mismatch: hub did not complete the proto 2 handshake within 3s");
        }, STAGE_TIMEOUT_MS.attaching);
      };
      socket.onmessage = (event) => {
        if (this.machineSocket !== socket) return;
        this.machineReadAt = Date.now();
        if (!this.machineSocketReady) {
          if (typeof event.data !== "string") {
            protocolFailure("Machine protocol mismatch: expected a proto 2 JSON handshake");
            return;
          }
          let hello: Record<string, any>;
          try { hello = JSON.parse(event.data) as Record<string, any>; }
          catch { protocolFailure("Machine protocol mismatch: hub returned an invalid handshake"); return; }
          if (hello.proto !== 2) {
            const supported = hello.error?.supported;
            protocolFailure(`Machine protocol mismatch: Cassy Cloud requires proto 2${supported ? `; hub supports ${supported}` : ""}`);
            return;
          }
          clearTimers();
          this.machineSocketReady = true;
          if (!settled) {
            settled = true;
            resolve(true);
          }
          socket.send(JSON.stringify({ channel: "events", subscribe: true }));
          for (const desired of this.desiredSessions) this.subscribeMachineSession(desired);
          return;
        }
        void this.handleMachineMessage(event.data, frameFence).catch(error => {
          if (!this.desired || this.machineSocket !== socket) return;
          if (error instanceof AuthenticationError) this.blockAuthentication(error.kind, error.message);
          else this.connectionLostNow(error instanceof Error ? error.message : "machine event failed");
        });
      };
      socket.onclose = (event) => {
        clearTimers();
        const wasReady = this.machineSocketReady;
        if (this.machineSocket === socket) this.machineSocket = undefined;
        this.machineSocketReady = false;
        this.machineSubscriptions.clear();
        this.healthPing = undefined;
        if (!settled) {
          settled = true;
          resolve(this.machineProtocolBlocked);
        }
        if (!this.desired || !wasReady || event.code === 1000 || this.machineProtocolBlocked) return;
        for (const desired of this.desiredSessions) {
          this.transitionAttach(desired, "failed", "dialing", { reason: "Machine terminal transport closed", cause: { code: "socket_closed", layer: "socket", retryable: true, closeCode: event.code } });
          this.callbacks.onSocketError(desired, "Machine terminal transport closed. Retrying…");
          this.scheduleAttach(desired);
        }
      };
      socket.onerror = () => {
        if (!this.machineSocketReady) return;
        for (const desired of this.desiredSessions) {
          this.transitionAttach(desired, "failed", "dialing", { reason: "machine terminal transport error" });
        }
      };
    });
  }

  private subscribeMachineSession(session: string): void {
    const socket = this.machineSocket;
    if (!this.machineSocketReady || !socket || socket.readyState !== WebSocket.OPEN) return;
    if (this.machineSubscriptions.has(session)) return;
    this.machineSubscriptions.add(session);
    this.transitionAttach(session, "attaching", "attaching");
    // Supervisors only (cas-6261): worker panes are never streamed to Commander.
    socket.send(JSON.stringify({ channel: `pty:${session}`, subscribe: true, workers: false }));
    const timeouts = this.attachTimeouts.get(session) ?? {};
    if (timeouts.ready !== undefined) window.clearTimeout(timeouts.ready);
    timeouts.ready = window.setTimeout(() => {
      if (!this.machineSocketReady || this.attachLifecycles.get(session)?.phase === "live") return;
      // cas-d15c (QA F01): the machine socket is still ready, so a session
      // whose stream the hub closed is still a session-only outage.
      this.transitionAttach(session, "failed", "attaching", { reason: "Machine stream sent no session state within 3s", sessionOnly: this.attachLifecycles.get(session)?.sessionOnly });
      this.callbacks.onSocketError(session, "Machine stream sent no session state within 3s. Retrying…");
      this.scheduleAttach(session);
    }, STAGE_TIMEOUT_MS.attaching);
    this.attachTimeouts.set(session, timeouts);
  }

  /**
   * A session a concurrent attach already brought onto the ready machine
   * socket (cas-8fe2). Two attaches can share one machine-socket opening; when
   * its ticket is refused, both handle the failure, and the slower one's
   * permission query can finish after a retry has opened the socket and
   * subscribed the session. Its failure is then stale: marking the session
   * failed and scheduling a retry would leave it "Reconnecting" for good,
   * because the retry finds the session already subscribed and changes nothing.
   */
  private servedByMachineSocket(session: string): boolean {
    return this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN && this.machineSubscriptions.has(session);
  }

  private async handleAttachFailure(session: string, error: unknown): Promise<void> {
    if (!this.desired) return;
    // Unsupported browser APIs need an upgrade, not another network retry.
    if (error instanceof UnsupportedBrowserError) {
      const stage = this.attachLifecycles.get(session)?.stage ?? "attaching";
      this.transitionAttach(session, "failed", stage, { reason: error.message, fatal: true });
      this.callbacks.onSocketError(session, error.message);
      return;
    }
    if (error instanceof AuthenticationError) {
      this.transitionAttach(session, "failed", "auth", { reason: error.message, authFailure: error.kind });
      this.blockAuthentication(error.kind, error.message, session);
      return;
    }
    if (this.servedByMachineSocket(session)) return;
    // Opaque errors are retried: a healthy public route cannot prove that
    // an authenticated route's browser-side failure revoked the pairing.
    const detail = error instanceof Error ? error.message : "unknown terminal attach failure";
    const cause = this.failureCause(error);
    const networkAccessHelp = isNetworkFailure(error) ? await localNetworkAccessHelp(this.machine.baseUrl, this.machine.label, permission => { cause.permission = permission; }) : undefined;
    if (!this.desired || this.servedByMachineSocket(session)) return;
    const failedStage = this.attachLifecycles.get(session)?.stage ?? "dialing";
    this.transitionAttach(session, "failed", failedStage, { reason: stageFailureDetail(failedStage, new URL(this.machine.baseUrl).host, detail), networkAccessHelp, cause });
    this.callbacks.onSocketError(session, `Terminal attach failed: ${detail}. Retrying…`);
    this.scheduleAttach(session);
  }

  private blockAuthentication(kind: AuthFailureKind, detail: string, session?: string): void {
    this.desired = false;
    this.stopHeartbeat();
    this.eventAbort?.abort();
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.clearAttachRetries();
    this.clearAttachTimeouts();
    this.machineSocket?.close(1000, "authentication blocked");
    this.machineSocket = undefined;
    this.machineSocketReady = false;
    this.machineSubscriptions.clear();
    this.healthPing = undefined;
    for (const socket of this.sockets.values()) socket.close(1000, "authentication blocked");
    this.sockets.clear();
    const cause = this.failureCause(new AuthenticationError(kind, detail));
    if (session) this.transitionAttach(session, "failed", "auth", { reason: detail, authFailure: kind, cause });
    this.transition("failed", "auth", { reason: detail, authFailure: kind, cause });
    this.callbacks.onAuthFailure?.(kind, detail);
    if (session) this.callbacks.onSocketError(session, "authentication blocked; re-pair to reconnect");
  }

  private scheduleAttach(session: string): void {
    if (!this.desired || this.attachRetryTimers.has(session)) return;
    const attempt = this.socketAttempts.get(session) ?? 0;
    // Capped like the machine connection's retry (cas-4ce5): a session held
    // down for a while must not wait 16–30 s to notice its machine is back.
    // HUB-J11 promises attempts at most 10 s apart.
    const delay = Math.min(MACHINE_RETRY_CEILING_MS, backoffDelay(attempt));
    this.socketAttempts.set(session, attempt + 1);
    const failed = this.attachLifecycles.get(session);
    this.transitionAttach(session, "backoff", failed?.stage ?? "dialing", { reason: failed?.reason, retryInMs: delay });
    const timer = window.setTimeout(() => {
      this.attachRetryTimers.delete(session);
      if (!this.desired) return;
      void this.attach(session);
    }, delay);
    this.attachRetryTimers.set(session, timer);
  }

  private clearAttachRetries(): void {
    for (const timer of this.attachRetryTimers.values()) window.clearTimeout(timer);
    this.attachRetryTimers.clear();
    this.socketAttempts.clear();
  }

  private startOpenTimeout(session: string, socket: WebSocket): void {
    this.clearAttachTimeouts(session);
    const timeouts = this.attachTimeouts.get(session) ?? {};
    timeouts.open = window.setTimeout(() => {
      if (this.sockets.get(session) !== socket || socket.readyState !== WebSocket.CONNECTING) return;
      this.timedOutSockets.add(socket);
      this.transitionAttach(session, "failed", "dialing", { reason: "Stuck dialing terminal — node may be offline (5s)" });
      this.callbacks.onSocketError(session, "Stuck dialing terminal — node may be offline (5s). Retrying…");
      socket.close();
    }, STAGE_TIMEOUT_MS.dialing);
    this.attachTimeouts.set(session, timeouts);
  }

  private startReadyTimeout(session: string, socket: WebSocket): void {
    const timeouts = this.attachTimeouts.get(session) ?? {};
    timeouts.ready = window.setTimeout(() => {
      if (this.sockets.get(session) !== socket || socket.readyState !== WebSocket.OPEN) return;
      this.timedOutSockets.add(socket);
      this.transitionAttach(session, "failed", "attaching", { reason: "Terminal opened but sent no session state within 3s" });
      this.callbacks.onSocketError(session, "Terminal attach opened but sent no session state within 3s. Retrying…");
      socket.close();
    }, STAGE_TIMEOUT_MS.attaching);
    this.attachTimeouts.set(session, timeouts);
  }

  private clearAttachTimeout(session: string, kind: "open" | "ready"): void {
    const timeouts = this.attachTimeouts.get(session);
    const timer = timeouts?.[kind];
    if (timer !== undefined) window.clearTimeout(timer);
    if (timeouts) delete timeouts[kind];
    if (timeouts && timeouts.open === undefined && timeouts.ready === undefined) this.attachTimeouts.delete(session);
  }

  private clearAttachTimeouts(session?: string): void {
    const clear = (key: string, timeouts: { open?: number; ready?: number } | undefined) => {
      if (!timeouts) return;
      if (timeouts.open !== undefined) window.clearTimeout(timeouts.open);
      if (timeouts.ready !== undefined) window.clearTimeout(timeouts.ready);
      this.attachTimeouts.delete(key);
    };
    if (session) {
      clear(session, this.attachTimeouts.get(session));
      return;
    }
    for (const [key, timeouts] of this.attachTimeouts) {
      clear(key, timeouts);
    }
  }

  /**
   * Heartbeats are going unanswered on a machine that still reads live
   * (cas-a6f0): the snapshot's `degraded`, the page's "Unsteady".
   */
  private unsteady(): boolean {
    return this.lifecycle.phase === "live" && this.missedHeartbeats >= DEGRADED_AFTER_MISSED_HEARTBEATS;
  }

  /**
   * Whether a supervisor message offered now would be held rather than put on
   * the wire: a probe waits on a doubted socket, or the machine is unsteady.
   */
  holdsMessages(): boolean {
    return this.probePingId !== undefined || this.unsteady();
  }

  /** Re-announce every live attach, so the page sends what it held (cas-0978, cas-a6f0). */
  private releaseHeldMessages(): void {
    for (const [session, snapshot] of this.attachLifecycles) {
      if (snapshot.phase === "live") this.callbacks.onAttachState?.(session, snapshot);
    }
  }

  send(session: string, message: unknown, clientRef?: string): boolean {
    // While a probe waits on the machine socket it may be half-open, and
    // while heartbeats go unanswered (unsteady) it may be dead: a message
    // sent into it could vanish, so it is refused here and the caller holds
    // it until the machine answers or the socket is replaced (cas-0978,
    // cas-a6f0, journey F9).
    // The journal's claim and credential reads are asynchronous. Recovery can
    // begin after the caller checked live, while its old socket is still OPEN.
    // Never write that pending send onto a socket recovery is about to replace.
    if (isSupervisorMessage(message) && (this.lifecycle.phase !== "live" || this.holdsMessages())) return false;
    const outbound = withClientRef(message, clientRef);
    if (this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN) {
      const resize = typeof outbound === "object" && outbound !== null && "ResizePane" in outbound;
      this.machineSocket.send(JSON.stringify(resize
        ? { channel: "resize", session, message: outbound }
        : { channel: `pty:${session}`, message: outbound }));
      return true;
    }
    const socket = this.sockets.get(session);
    if (!socket || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(outbound));
    const sent = sendMessageClientRef(outbound);
    if (sent) this.legacySends.set(socket, [...(this.legacySends.get(socket) ?? []), sent]);
    return true;
  }

  /**
   * A retryable refusal: the hub closes this session's stream right after it
   * and the page attaches again. Repeated refusals back off that reattach
   * (cas-a355): the n-th refusal since the last acknowledged send waits
   * backoffDelay(min(n - 1, 3)), about 1, 2, 4 then 8 s.
   */
  private unansweredAfter(session: string, clientRef: string): string[] {
    const socket = this.sockets.get(session);
    const written = socket ? this.legacySends.get(socket) ?? [] : [];
    const index = written.indexOf(clientRef);
    if (!socket || index < 0) return [];
    this.legacySends.set(socket, written.slice(0, index));
    return written.slice(index + 1);
  }

  private noteUpstreamRefusal(session: string): void {
    // Refused again right after going live: the upstream is still gone.
    this.cancelUpstreamStreakReset(session);
    const streak = (this.upstreamRefusalStreak.get(session) ?? 0) + 1;
    this.upstreamRefusalStreak.set(session, streak);
    this.socketAttempts.set(session, Math.min(streak - 1, UPSTREAM_BACKOFF_MAX_ATTEMPT));
  }

  /**
   * The session went live. Every reattach while the upstream is gone also
   * goes live and is refused again as soon as the held send goes out, so live
   * alone proves nothing. A session that stays live for
   * UPSTREAM_STREAK_SETTLE_MS with no retryable refusal has its upstream
   * back, and the next refusal starts the backoff at 1 s again, delivered
   * send or not (cas-2036). Before this, a held send that expired unsent
   * left the streak in place, and the next drop, however much later, started
   * at 8 s.
   */
  private settleUpstreamStreak(session: string): void {
    if (!this.upstreamRefusalStreak.has(session)) return;
    this.cancelUpstreamStreakReset(session);
    this.upstreamStreakResets.set(session, window.setTimeout(() => {
      this.upstreamStreakResets.delete(session);
      this.upstreamRefusalStreak.delete(session);
    }, UPSTREAM_STREAK_SETTLE_MS));
  }

  private cancelUpstreamStreakReset(session: string): void {
    const pending = this.upstreamStreakResets.get(session);
    if (pending === undefined) return;
    window.clearTimeout(pending);
    this.upstreamStreakResets.delete(session);
  }

  private clearUpstreamStreak(session: string): void {
    this.cancelUpstreamStreakReset(session);
    this.upstreamRefusalStreak.delete(session);
  }

  requestPaneKeyframe(session: string, paneId: string): boolean {
    const key = `${session}:${paneId}`;
    if (this.keyframeRequests.has(key)) return true;
    if (!this.send(session, { RequestPaneKeyframe: { pane_id: paneId } })) return false;
    this.keyframeRequests.add(key);
    return true;
  }

  /** Request a private, device-scoped page of durable Commander turns. */
  requestConversationHistory(session: string, before?: number, limit = 50): boolean {
    return this.send(session, {
      ConversationHistoryRequest: {
        request_id: crypto.randomUUID(),
        ...(before === undefined ? {} : { before }),
        limit: Math.min(50, Math.max(1, limit)),
        // The hub overwrites this from the authenticated credential. Including
        // it here keeps direct daemon fixtures and the wire shape explicit.
        device_id: this.machine.deviceId,
      },
    });
  }

  private async handleMachineMessage(input: string | ArrayBuffer | Blob, frameFence = credentialFence(this.machine)): Promise<void> {
    if (typeof input !== "string") {
      const bytes = new Uint8Array(input instanceof Blob ? await input.arrayBuffer() : input);
      if (bytes.length < 9 || new TextDecoder().decode(bytes.subarray(0, 4)) !== "CAS2") return;
      const kind = bytes[4];
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      const sessionLength = view.getUint16(5);
      const paneLength = view.getUint16(7);
      const payloadStart = 9 + sessionLength + paneLength;
      if (payloadStart > bytes.length) return;
      const decoder = new TextDecoder();
      const session = decoder.decode(bytes.subarray(9, 9 + sessionLength));
      const pane = decoder.decode(bytes.subarray(9 + sessionLength, payloadStart));
      const payload = bytes.slice(payloadStart);
      if (kind === 1) this.callbacks.onOutput(session, pane, payload);
      else if (kind === 2) {
        this.keyframeRequests.delete(`${session}:${pane}`);
        this.callbacks.onPaneKeyframe(session, pane, payload);
      }
      return;
    }
    let envelope: Record<string, any>;
    try { envelope = JSON.parse(input) as Record<string, any>; }
    catch { return; }
    if (envelope.channel === "health" && typeof envelope.pong === "number") {
      if (envelope.pong === this.probePingId) {
        // The doubted socket answered: messages held meanwhile can go now.
        this.probePingId = undefined;
        if (!this.unsteady()) this.releaseHeldMessages();
      }
      if (this.healthPing?.id !== envelope.pong) return;
      const latencyMs = Math.max(0, Math.round(performance.now() - this.healthPing.startedAt));
      this.healthPing = undefined;
      const wasUnsteady = this.unsteady();
      this.missedHeartbeats = 0;
      this.lastHeartbeatAt = Date.now();
      this.transition("live", "live", { latencyMs });
      this.callbacks.onLatency?.(latencyMs);
      // cas-a6f0: the unsteady machine answered again; what it held goes now.
      if (wasUnsteady && this.probePingId === undefined) this.releaseHeldMessages();
      return;
    }
    if (envelope.channel === "events" && envelope.event) {
      this.deliverMachineEvent(envelope.event as Record<string, unknown>);
      void this.eventCatalog.request();
      return;
    }
    const session = typeof envelope.channel === "string" && envelope.channel.startsWith("pty:")
      ? envelope.channel.slice(4) : undefined;
    if (!session) return;
    if (envelope.keyframe_required) {
      for (const key of this.keyframeRequests) {
        if (key.startsWith(`${session}:`)) this.keyframeRequests.delete(key);
      }
      this.callbacks.onFlowControlReset?.(session);
      for (const pane of this.sessionPanes.get(session) ?? []) this.requestPaneKeyframe(session, pane.id);
      return;
    }
    if (envelope.closed) {
      this.machineSubscriptions.delete(session);
      this.transitionAttach(session, "failed", "attaching", { reason: "Session daemon stream closed", sessionOnly: true });
      this.callbacks.onSocketError(session, "Session daemon stream closed. Retrying…");
      this.scheduleAttach(session);
      return;
    }
    if (envelope.error) {
      const detail = String(envelope.error.message ?? envelope.error.code ?? "machine protocol error");
      const rejection = messageRejection(envelope.error);
      // A retryable refusal is followed by the hub closing this session's
      // stream (`closed` above), which reattaches it; the held send goes out
      // on that live attach (cas-0653), after a backoff (cas-a355).
      if (rejection.retryable) this.noteUpstreamRefusal(session);
      if (typeof envelope.error.client_ref === "string") this.callbacks.onMessageRejected?.(session, envelope.error.client_ref, detail, rejection);
      else this.callbacks.onSocketError(session, detail);
      return;
    }
    if (envelope.message) this.handleDaemonObject(session, envelope.message as Record<string, any>, frameFence);
  }

  private recordEventRecovery(code: "event_sequence_gap" | "event_retention_gap" | "event_epoch_changed" | "viewer_lagged", requestId?: string): void {
    this.transition(this.lifecycle.phase, this.lifecycle.stage, { cause: { code, layer: "events", retryable: true, requestId } });
  }

  private deliverMachineEvent(event: Record<string, unknown>, replaying = false): void {
    if (event.kind === "viewer_lagged") {
      const requestId = typeof event.request_id === "string" ? event.request_id : undefined;
      this.recordEventRecovery("viewer_lagged", requestId);
      this.eventAbort?.abort(new EventRecoveryError("viewer_lagged", requestId));
      return;
    }
    const sequence = Number(event.sequence ?? 0);
    if (Number.isFinite(sequence) && sequence > 0) {
      const result = this.eventRecovery.accept(sequence, Number(event.revision ?? 0));
      if (!result.deliver) return;
      if (result.gap && !replaying) {
        this.recordEventRecovery("event_sequence_gap");
        // Reconnect replays retained rows, including revisions, without losing
        // the triggering live event. Deliver it before reopening the stream.
        this.callbacks.onMachineEvent(event);
        this.eventAbort?.abort(new EventRecoveryError("event_sequence_gap"));
        return;
      }
    }
    this.callbacks.onMachineEvent(event);
  }

  private async handleDaemonMessage(session: string, input: string | ArrayBuffer | Blob, frameFence = credentialFence(this.machine)): Promise<void> {
    const text = typeof input === "string" ? input : input instanceof Blob ? await input.text() : new TextDecoder().decode(input);
    const message = JSON.parse(text) as Record<string, any>;
    this.handleDaemonObject(session, message, frameFence);
  }

  private handleDaemonObject(session: string, message: Record<string, any>, frameFence = credentialFence(this.machine)): void {
    if (message.Welcome) {
      const socket = this.sockets.get(session);
      if (socket) {
        this.readySockets.add(socket);
        this.clearAttachTimeout(session, "ready");
        this.socketAttempts.set(session, 0);
        this.settleUpstreamStreak(session);
        this.transitionAttach(session, "live", "live");
      } else if (this.machineSocketReady) {
        this.clearAttachTimeout(session, "ready");
        this.socketAttempts.set(session, 0);
        this.settleUpstreamStreak(session);
        this.transitionAttach(session, "live", "live");
      }
      // A fresh session can recover before the event-stream retry timer.
      // Check the machine now; the session alone never bypasses the machine
      // send fence, and connect still replaces untrusted recovery sockets.
      if (this.attachLifecycles.get(session)?.phase === "live"
        && (this.lifecycle.phase === "backoff" || this.lifecycle.phase === "failed")) this.networkChanged();
      const welcome = message.Welcome;
      this.sessionPanes.set(session, welcome.state.panes);
      const authoritative = Number(welcome.protocol_version ?? 1) >= 3
        && Array.isArray(welcome.capabilities)
        && welcome.capabilities.includes("authoritative_pane_keyframes");
      // Protocol v3 daemons know the request even when a rolling hub relay
      // drops the additive capability list. Keep the capability check for
      // older v3 peers, but do not let that metadata omission make a durable
      // conversation disappear on reopen.
      const protocolVersion = Number(welcome.protocol_version ?? 1);
      if (protocolVersion >= 3 || (Array.isArray(welcome.capabilities) && welcome.capabilities.includes("conversation_history"))) {
        if (this.requestConversationHistory(session)) this.callbacks.onConversationHistoryRequested?.(session);
      } else {
        this.callbacks.onConversationHistoryUnavailable?.(session);
      }
      for (const key of this.keyframeRequests) {
        if (key.startsWith(`${session}:`)) this.keyframeRequests.delete(key);
      }
      if (authoritative) {
        // The metadata identifies roles, so supervisor content is requested in
        // the first client turn; mounted workers are requested by the renderer.
        const supervisor = welcome.state.panes.find((pane: PaneInfo) => pane.kind === "Supervisor");
        if (supervisor) this.requestPaneKeyframe(session, supervisor.id);
        this.callbacks.onSessionState(session, welcome.state, undefined, true);
      } else {
        this.callbacks.onSessionState(session, welcome.state, welcome.scrollback, false);
      }
    } else if (message.PaneKeyframe) {
      const keyframe = message.PaneKeyframe;
      this.keyframeRequests.delete(`${session}:${keyframe.pane_id}`);
      this.callbacks.onPaneKeyframe(session, keyframe.pane_id, new Uint8Array(keyframe.ansi));
    } else if (message.PaneSize) {
      const size = message.PaneSize;
      this.callbacks.onPaneSize?.(session, size.pane_id, size.cols, size.rows, String(size.authority));
    } else if (message.StateUpdate) {
      this.sessionPanes.set(session, message.StateUpdate.state.panes);
      this.callbacks.onSessionState(session, message.StateUpdate.state);
    } else if (message.Output) {
      this.callbacks.onOutput(session, message.Output.pane_id, new Uint8Array(message.Output.data));
    } else if (message.MessageQueued) {
      const queued = messageQueuedFromDaemon(message);
      // A send reached the daemon: the upstream is back, so the next
      // retryable refusal starts the backoff afresh (cas-a355).
      if (queued) this.clearUpstreamStreak(session);
      if (queued) this.callbacks.onMessageQueued?.(session, queued, frameFence);
    } else if (message.OperatorReply) {
      this.callbacks.onOperatorReply?.(session, message.OperatorReply as OperatorReply, frameFence);
    } else if (message.OperatorNoticeResolved) {
      this.callbacks.onOperatorNoticeResolved?.(session, message.OperatorNoticeResolved as OperatorNoticeResolved);
    } else if (message.OperatorMessage) {
      this.callbacks.onOperatorMessage?.(session, message.OperatorMessage as ConversationHistoryMessage);
    } else if (message.ConversationHistory) {
      this.callbacks.onConversationHistory?.(session, message.ConversationHistory as ConversationHistoryPage, frameFence);
    } else if (message.SessionSummary) {
      this.callbacks.onSessionSummary?.(session, message.SessionSummary.summary);
    } else if (message.PaneAdded || message.PaneRemoved || message.PaneExited) {
      this.send(session, "GetState");
    } else if (message.error) {
      // The legacy socket puts the operator text beside the code (cas-a0e2).
      const detail = typeof message.error === "string"
        ? (typeof message.message === "string" ? message.message : message.error)
        : String(message.error.message ?? message.error.code ?? "Message refused");
      const clientRef = message.client_ref ?? (typeof message.error === "object" ? message.error.client_ref : undefined);
      const rejection = messageRejection(message.error, message);
      // The hub closes the legacy socket after a retryable refusal, and its
      // close handler reattaches (cas-0653).
      if (typeof clientRef === "string") {
        if (rejection.retryable) this.noteUpstreamRefusal(session);
        this.callbacks.onMessageRejected?.(session, clientRef, detail, rejection);
        // The hub stops reading this socket after the refusal (hub/server.rs
        // `proxy_socket`), so a message written after the refused one never
        // reached the daemon and is never answered. It is refused the same
        // way, and held again, instead of waiting out "Not confirmed"
        // (cas-a355).
        if (rejection.retryable) for (const later of this.unansweredAfter(session, clientRef)) this.callbacks.onMessageRejected?.(session, later, detail, rejection);
      } else this.callbacks.onSocketError(session, detail);
    } else if (message.Error) {
      if (typeof message.Error.client_ref === "string") this.callbacks.onMessageRejected?.(session, message.Error.client_ref, message.Error.message);
      else this.callbacks.onSocketError(session, message.Error.message);
    }
  }
}

/** Normalize the additive daemon acknowledgment before invoking the callback. */
export function messageQueuedFromDaemon(message: Record<string, any>): MessageQueued | undefined {
  const value = message.MessageQueued;
  if (!value || typeof value !== "object") return undefined;
  if (!Number.isFinite(Number(value.notification_id)) || typeof value.target !== "string") return undefined;
  return {
    client_ref: typeof value.client_ref === "string" ? value.client_ref : null,
    notification_id: Number(value.notification_id),
    target: value.target,
    stamped: value.stamped === true,
    ...(typeof value.device_label === "string" ? { device_label: value.device_label } : {}),
  };
}

function isSupervisorMessage(message: unknown): boolean {
  return typeof message === "object" && message !== null && "SendMessage" in message;
}

/** Four heartbeat windows: a recently speaking terminal survives event-only recovery. */
export const ATTACH_LIVENESS_MS = HEARTBEAT_INTERVAL_MS * RECONNECT_AFTER_MISSED_HEARTBEATS;
/** Event headers alone must not reset exponential reconnect backoff. */
export const EVENT_STREAM_STABLE_MS = 10_000;

/** Reattach attempts after repeated `upstream_unavailable` refusals stop growing here: backoffDelay(3), about 8 s (cas-a355). */
export const UPSTREAM_BACKOFF_MAX_ATTEMPT = 3;

/**
 * How long a session must stay live, with no retryable refusal, before the
 * refusal streak resets (cas-2036). Longer than a held send's resend and its
 * refusal take after a live attach, so a still-missing upstream keeps
 * backing off.
 */
export const UPSTREAM_STREAK_SETTLE_MS = 10_000;

/** The client_ref of an outbound SendMessage, if it carries one. */
function sendMessageClientRef(message: unknown): string | undefined {
  if (typeof message !== "object" || message === null) return undefined;
  const send = (message as Record<string, unknown>).SendMessage;
  const ref = send && typeof send === "object" ? (send as Record<string, unknown>).client_ref : undefined;
  return typeof ref === "string" ? ref : undefined;
}

function withClientRef(message: unknown, clientRef: string | undefined): unknown {
  if (!clientRef || typeof message !== "object" || message === null) return message;
  const envelope = message as Record<string, unknown>;
  const send = envelope.SendMessage;
  if (!send || typeof send !== "object") return message;
  return { ...envelope, SendMessage: { ...(send as Record<string, unknown>), client_ref: clientRef } };
}
