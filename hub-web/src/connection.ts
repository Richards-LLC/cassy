import type { CommandReceipt } from "./supervisor-commands";
import type { AccountChange, AccountsCatalog } from "./accounts-model";
import { installationLock, notifyInstallation } from "./installation-access";
import { catalog, installationStore } from "./storage";
import { credentialFence, type CredentialFence } from "./commander-journal";
import { anySignal } from "./abort-signals";
import { withRequestDeadline } from "./request-deadline";
import { EventRecovery } from "./event-recovery";
import { ConnectionDiagnostics, httpRefusalCause, type CauseEvidence } from "./connection-diagnostics";
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
  SESSION_RETRY_CEILING_MS,
  RECONNECT_AFTER_MISSED_HEARTBEATS,
  SOCKET_PROBE_TIMEOUT_MS,
  stageFailureDetail,
  STAGE_TIMEOUT_MS,
  type ConnectionPhase,
  type ConnectionSnapshot,
  type ConnectionStage,
  type AttachSnapshot,
} from "./connection-state";
import type { ConversationHistoryMessage, ConversationHistoryPage, HubSession, LeaseState, MessageQueued, OperatorNoticeResolved, OperatorReply, SessionCardSummary, SessionState, StoredMachine } from "./types";

import { sessionsPath } from "./worker-visibility";
import { dormantRevealed } from "./dormant-visibility";
import type { PaneBootstrap, ScrollbackPage } from "./raw-output";
import { workEventDetail, workEventSupport, workEventsPage, type WorkEventDetail, type WorkEventsPage, type WorkEventSupport } from "./work-events";

export type ConnectionState = ConnectionSnapshot;

// Leave a retired writer readable briefly for its in-flight receipts. Calling
// close() immediately discards those frames even with onmessage installed.
const LEGACY_RECEIPT_DRAIN_MS = 5_000;
// A short-lived SSE stream can flap while its terminal socket still speaks.
// Keep that event-only retry backoff separate from actual outage recovery.
const EVENT_RETRY_CEILING_MS = 10_000;

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
  onSupervisorCommandSupport?(session: string, supported: boolean): void;
  onSupervisorCommandReceipt?(session: string, receipt: CommandReceipt): void;
  onState(state: ConnectionState): void;
  onAttachState?(session: string, state: AttachSnapshot): void;
  onLatency?(latencyMs: number): void;
  /** The browser hinted at a changed network: back online, a new connection type, or woken from sleep (cas-0b3d). */
  onNetworkChange?(): void;
  onAuthFailure?(kind: AuthFailureKind, detail: string): void;
  onCredentialRefreshed?(machine: StoredMachine): Promise<void> | void;
  onMachineInfo?(machine: HubMachineInfo | undefined): void;
  onSessions(sessions: HubSession[], freshnessThresholdSecs?: number): void;
  onMachineEvent(event: Record<string, unknown>): void;
  /** `scrollback` is a pre-v3 daemon's raw pane bytes; a v3 daemon sends pane_bootstrap instead (cas-f285). */
  onSessionState(session: string, state: SessionState, scrollback?: Record<string, number[][]>): void;
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
  /** A work-events page this client asked for (cas-ff22). */
  onWorkEvents?(session: string, page: WorkEventsPage): void;
  onWorkEventDetail?(session: string, detail: WorkEventDetail): void;
  /** A work-events request was refused or failed; `requestId` is its id. */
  onWorkEventsFailed?(session: string, requestId: string, detail: string): void;
  /** The daemon's stream gained events since this client read it (cas-ff22). */
  onWorkEventsAppended?(session: string): void;
  /** A page of the pane's rendered rows, answering requestScrollback (cas-f285). */
  onScrollbackPage?(session: string, page: ScrollbackPage): void;
  /** The hub dropped output for this session under backpressure: rows read before may be stale. */
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
 * pairing: an expired credential refreshes (cas-d636). A bare 401 names no
 * reason, so it needs pairing but is never a revocation: a hub that failed
 * internally once answered that way, and "revoked" purged and permanently
 * fenced a pairing the hub still accepted (cas-2904).
 */
export function authFailureKind(status: number, refusal: AuthRefusal, expiresAt: string, now = Date.now()): AuthFailureKind {
  if (status === 403) return "scope-mismatch";
  if (refusal.reason === "expired") return "expired";
  if (refusal.reason === undefined) return Date.parse(expiresAt) <= now ? "expired" : "needs-pairing";
  return "revoked";
}

function authFailureMessage(kind: AuthFailureKind, refusal: AuthRefusal): string {
  if (kind === "expired") return "pairing expired";
  if (kind === "scope-mismatch") return "credential ceiling does not grant this operation";
  if (refusal.reason === "key_mismatch") return "this browser's key no longer matches the pairing";
  if (refusal.reason === "idle") return "pairing unused for too long";
  if (refusal.reason === "origin_mismatch") return "pairing belongs to another Cassy Cloud";
  if (kind === "needs-pairing") return "the machine refused this pairing without a reason";
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
      const signal = anySignal([stream?.signal ?? new AbortController().signal, AbortSignal.timeout(SOCKET_PROBE_TIMEOUT_MS)]);
      // A heartbeat/manual read may predate this event. Joining that flight
      // alone swallows the invalidation (a stopped worker stays in the roster).
      // Drain it first, then read the changed catalog within the same deadline.
      // A drained read that fails or times out is the older read's loss, not
      // this event's (cas-1966): only a refused pairing skips the fresh read.
      if (this.catalogRequest) {
        try {
          await this.refreshSessions(signal);
        } catch (error) {
          if (error instanceof AuthenticationError) throw error;
        }
      }
      if (!this.desired || this.lifecycle.phase !== "live" || stream !== this.eventAbort) return;
      await this.refreshSessions(signal);
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
  /** Each session's per-pane attach metadata from its last Welcome (cas-f285). */
  private readonly paneBootstraps = new Map<string, PaneBootstrap[]>();
  /** What each attached session's daemon offers of the work-event stream (cas-ff22). */
  private readonly workEventSupport = new Map<string, WorkEventSupport>();
  /** Work-event request ids in flight, so their refusals are not read as sends. */
  private readonly workEventRequests = new Set<string>();
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
  private readonly legacySends = new WeakMap<WebSocket, Array<{ clientRef: string; target: string }>>();
  private readonly retiredLegacySockets = new Map<WebSocket, number>();
  private readonly attachRetryTimers = new Map<string, number>();
  /** Sessions with an attach() between its start and its socket (cas-5dbc). */
  private readonly attachesInFlight = new Map<string, number>();
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
  private readonly endedSessions = new Set<string>();
  private readonly machineSubscriptions = new Set<string>();
  private healthPing?: { id: number; startedAt: number };
  private attachStarts: number[] = [];
  private attachRateQueue = Promise.resolve();
  private attachRateAbort = new AbortController();
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
    this.resetAttachRate();
    this.clearAttachTimeouts();
    for (const session of [...this.upstreamStreakResets.keys()]) this.cancelUpstreamStreakReset(session);
    this.machineSocket?.close(1000, "machine removed");
    this.machineSocket = undefined;
    this.machineSocketReady = false;
    this.machineSocketOpening = undefined;
    this.desiredSessions.clear();
    this.machineSubscriptions.clear();
    this.paneBootstraps.clear();
    this.healthPing = undefined;
    for (const socket of this.sockets.values()) socket.close(1000, "machine removed");
    this.sockets.clear();
    for (const socket of this.retiredLegacySockets.keys()) this.closeRetiredLegacySocket(socket);
    for (const session of this.attachLifecycles.keys()) {
      this.transitionAttach(session, "idle", "idle");
      this.attachLifecycles.delete(session);
    }
    this.transition("idle", "idle");
  }

  snapshot(): ConnectionSnapshot { return this.lifecycle; }

  attachSnapshot(session: string): AttachSnapshot | undefined { return this.attachLifecycles.get(session); }

  attachSnapshots(): ReadonlyMap<string, AttachSnapshot> { return this.attachLifecycles; }

  /** Current daemon traffic can stay live while only event delivery retries. */
  hasLiveAttach(session: string): boolean {
    if (!this.desired || this.lifecycle.fatal || this.lifecycle.authFailure || this.lifecycle.networkAccessHelp
      || this.missedHeartbeats >= RECONNECT_AFTER_MISSED_HEARTBEATS
      || this.attachLifecycles.get(session)?.phase !== "live") return false;
    const socket = this.sockets.get(session);
    const lastRead = socket ? this.legacyReadAt.get(socket) : this.machineReadAt;
    const ready = socket
      ? socket.readyState === WebSocket.OPEN && this.readySockets.has(socket)
      : this.servedByMachineSocket(session);
    return ready && lastRead !== undefined && Date.now() - lastRead < ATTACH_LIVENESS_MS;
  }

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
        await this.withStageTimeout("auth", signal => this.refreshMachineInfo(signal));
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
      // Catalog freshness is independent of authentication and transport.
      this.refreshCatalog();
      this.startHeartbeat();
      // Event-stream loss alone does not condemn terminal transports that
      // still answer. A proved machine outage already abandoned them.
      if (recovering) this.reattachDesired("Reconnected after the network changed");
      else this.ensureAttachesPending();
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
      this.resetAttachRate();
      this.clearAttachRetryTimers();
      this.connectionLost = true;
      this.resumeStage = stage;
      const reason = error instanceof Error ? error.message : "unknown connection failure";
      const target = new URL(this.machine.baseUrl).host;
      const cause = this.failureCause(error);
      this.transition("failed", stage, { reason: stageFailureDetail(stage, target, reason), cause });
      // Capped well below the backoff's 30 s ceiling: a network that returns
      // without an event (Tailscale switched on) still has time to reattach
      // within the five-second recovery window.
      const ceiling = liveSince === undefined ? MACHINE_RETRY_CEILING_MS : EVENT_RETRY_CEILING_MS;
      const delay = Math.min(ceiling, backoffDelay(this.attempt++));
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
    if (error instanceof HubRequestError) return { code: httpRefusalCause(error.status, error.code, error.detail), layer: "http", retryable: true, status: error.status, requestId: error.requestId };
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
    let signedWith = this.machine.credentialId;
    const send = async () => {
      signedWith = this.machine.credentialId;
      const proof = await dpopHeaders(this.machine, method, htu, Date.now() + this.clockOffsetMs);
      init.signal?.throwIfAborted();
      if (path === "/v1/auth/websocket-ticket" && !await this.waitForAttachSlot(init.signal)) {
        throw new DOMException("Attachment cancelled", "AbortError");
      }
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
      // commit/rollback, then retry only a different accepted credential. A
      // concurrent request may already have adopted it while this one waited
      // for the lock (cas-d308: the inventory read and a catalog poll both
      // left with the stale credential, and only the adopter retried).
      const adopted = await installationLock(this.machine.id, () => this.adoptInstallation(), init.signal ?? undefined);
      if (adopted || this.machine.credentialId !== signedWith) response = await send();
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
    // The hub refuses any future iat, so even sub-second corrections matter.
    // Its integer second also rounds the retry down conservatively.
    this.clockOffsetMs = serverTime * 1000 - Date.now();
  }

  uploadFile(session: string, id: string, name: string, data: string): Promise<{ path: string; artifact_id: string; mime: string }> {
    // Budget the base64 wire payload at 64 KiB/s (512 kbit/s), plus signing,
    // JSON overhead and hub storage time. Ordinary reads keep their short deadline.
    const timeoutMs = 10_000 + Math.ceil(data.length / 65_536) * 1000;
    return this.request("POST", `/v1/sessions/${encodeURIComponent(session)}/uploads/${id}`, { name, data }, undefined, timeoutMs);
  }

  async request<T>(method: string, path: string, body?: unknown, signal?: AbortSignal, timeoutMs?: number): Promise<T> {
    return withRequestDeadline(async boundedSignal => {
    const startedAt = performance.now();
    const { response, refusal } = await this.authorizedFetch(method, path, {
      headers: body === undefined ? {} : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: boundedSignal,
    });
    if (response.status === 403) {
      const error = await hubRequestError(method, path, response.clone());
      if (error.code === "session_denied") throw error;
    }
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
    }, signal, timeoutMs);
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
      this.acceptMachineInfo(info);
    } catch (error) {
      if (error instanceof AuthenticationError || error instanceof TransientAuthError) throw error;
      // Only a missing endpoint proves an old hub. A timeout or busy store
      // cannot revoke capabilities this paired hub already advertised.
      if (error instanceof HubRequestError && error.status === 404) {
        this.acceptMachineInfo({ schema_version: 1, version: "unknown", capabilities: [] });
      } else throw error;
    }
  }

  private acceptMachineInfo(info: HubMachineInfo): void {
    this.machineMultiplex = info.capabilities.includes("machine_multiplex_v2");
    this.callbacks.onMachineInfo?.(info);
  }

  /**
   * A short-lived signed URL for viewing an artifact the session published
   * (cassy#910). Unlike `request`, a refusal is an answer, not an exception:
   * the machine's stable error code tells Commander what to say.
   */
  /** Mint a short-lived, revocable media capability; never put pairing credentials in src. */
  async artifactPreview(session: string, artifactId: string): Promise<ArtifactViewResult> {
    return withRequestDeadline(async signal => {
      const path = `/v1/sessions/${encodeURIComponent(session)}/artifacts/${encodeURIComponent(artifactId)}`;
      const { response } = await this.authorizedFetch("POST", `${path}/ticket`, { signal });
      const body = await response.json().catch(() => undefined) as Record<string, unknown> | undefined;
      if (response.ok && typeof body?.ticket === "string" && body.ticket.length > 0 && body.ticket.length <= 4_096 && typeof body.expires_at === "string" && Number.isFinite(Date.parse(body.expires_at))) {
        const url = new URL(`${path}/content`, this.machine.baseUrl);
        url.searchParams.set("t", body.ticket);
        // Convert the hub's clock to this browser's clock for the expiry fast path.
        const expiresAt = new Date(Date.parse(body.expires_at) - this.clockOffsetMs).toISOString();
        return { ok: true, view: { artifact_id: artifactId, url: url.href, expires_at: expiresAt } };
      }
      return { ok: false, status: response.ok ? 502 : response.status, code: typeof body?.error === "string" ? body.error : "preview_unavailable" };
    });
  }

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

  /**
   * A short-lived URL for an HTML page the session published, served by this
   * machine under its sandbox policy (cas-3aff). Commander frames it inline
   * and opens it in a tab; the page itself never enters this document.
   */
  async visualPage(session: string, artifactId: string): Promise<ArtifactViewResult> {
    return withRequestDeadline(async signal => {
    const path = `/v1/sessions/${encodeURIComponent(session)}/artifacts/${encodeURIComponent(artifactId)}/page`;
    const { response } = await this.authorizedFetch("POST", path, { signal });
    const body = await response.json().catch(() => undefined) as Record<string, unknown> | undefined;
    // A frame is only ever pointed at this paired machine's own origin.
    const url = response.ok && typeof body?.url === "string" ? new URL(body.url, this.machine.baseUrl) : undefined;
    if (url && url.origin === new URL(this.machine.baseUrl).origin) {
      return { ok: true, view: { ...(body as unknown as ArtifactView), url: url.href } };
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

  /** Registry-backed machine accounts; management uses the hub's allowlist. */
  async accounts(signal?: AbortSignal): Promise<AccountsCatalog> {
    return this.request("GET", "/v1/accounts", undefined, signal);
  }

  async changeAccount(change: AccountChange): Promise<void> {
    await this.request("POST", "/v1/accounts/operations", change);
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

  /**
   * Grant or revoke a task's write access outside its worktree (cas-ab04,
   * GH #1169). Needs factory:manage; the hub records the grant with this
   * device as its source and posts a receipt into the conversation.
   */
  async writeGrant(session: string, body: Readonly<Record<string, unknown>>): Promise<Record<string, unknown>> {
    return this.request("POST", `/v1/sessions/${encodeURIComponent(session)}/write-grants`, body);
  }

  /**
   * Answer an agent's secret request (cas-1996). Needs factory:manage. The
   * hub writes the value to a private file on its machine and replies to the
   * supervisor with the path; the value never travels as a message.
   */
  async answerSecret(session: string, notificationId: number, answer: { value: string } | { decline: true }): Promise<{ name?: string; path?: string; saved?: boolean; declined?: boolean }> {
    return this.request("POST", `/v1/sessions/${encodeURIComponent(session)}/secrets/${notificationId}`, answer);
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

  /** Report this browser's usage counts (cas-0b3d); the hub keeps the highest count per day and action. */
  async postUsage(entries: readonly { day: string; action: string; journey: string; count: number }[]): Promise<void> {
    await this.request("POST", "/v1/usage", { entries });
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

  private refreshCatalog(): void {
    void this.refreshSessions(AbortSignal.timeout(3_000)).catch(error => {
      if (error instanceof AuthenticationError && this.desired && this.lifecycle.phase === "live") {
        this.blockAuthentication(error.kind, error.message);
      }
    });
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
    this.ensureAttachesPending();
    const started = performance.now();
    // Catalog reads can wait on project stores while the machine socket is
    // healthy. Refresh the rows independently: their deadline says nothing
    // about transport liveness. A refused pairing still ends the connection.
    this.refreshCatalog();
    try {
      if (this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN) {
        if (this.healthPing) {
          this.missedHeartbeats += 1;
          this.transition("live", "live", { reason: "machine WebSocket heartbeat missed", cause: { code: "request_timeout", layer: "socket", retryable: true } });
          if (this.missedHeartbeats >= RECONNECT_AFTER_MISSED_HEARTBEATS) {
            // The hub answers HTTP but not this socket: it is half-open. A
            // close() on it can wait out a closing handshake that never
            // comes, so it is dropped and replaced now (cas-0978).
            try {
              await withRequestDeadline(signal => this.probeHealth(signal), undefined, SOCKET_PROBE_TIMEOUT_MS);
            } catch {
              if (this.desired && this.lifecycle.phase === "live" && this.healthPing) this.connectionLostNow("The socket and health endpoint stopped answering");
              return;
            }
            if (!this.desired || this.lifecycle.phase !== "live" || !this.healthPing) return;
            this.missedHeartbeats = 0;
            this.transition("live", "live");
            this.reattachDesired("Machine terminal transport stopped answering", true);
            return;
          }
          // Keep the outstanding id through the grace period. Replacing it
          // every tick discarded a delayed but valid pong under host load.
          return;
        }
        const id = Date.now();
        this.healthPing = { id, startedAt: started };
        this.machineSocket.send(JSON.stringify({ channel: "health", ping: id }));
        return;
      }
      await withRequestDeadline(signal => this.probeHealth(signal), undefined, SOCKET_PROBE_TIMEOUT_MS);
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
   * for minutes. Reset its state here; only known send receipts can still
   * settle after retirement, never transport state.
   */
  private abandonSockets(reason: string): void {
    this.resetAttachRate();
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
    for (const socket of this.sockets.values()) this.retireLegacySocket(socket);
    this.sockets.clear();
    this.clearAttachTimeouts();
    for (const session of this.desiredSessions) {
      const retry = this.attachRetryTimers.get(session);
      if (retry !== undefined) window.clearTimeout(retry);
      this.attachRetryTimers.delete(session);
      this.transitionAttach(session, "failed", "dialing", { reason });
    }
  }

  private retireLegacySocket(socket: WebSocket): void {
    socket.onopen = null; socket.onerror = null; socket.onclose = null;
    if (socket.readyState === WebSocket.OPEN && this.legacySends.get(socket)?.length) {
      // Retire from the send path, keeping only known receipts readable.
      this.retiredLegacySockets.set(socket, window.setTimeout(() => this.closeRetiredLegacySocket(socket), LEGACY_RECEIPT_DRAIN_MS));
      socket.onclose = () => this.closeRetiredLegacySocket(socket);
    } else this.closeRetiredLegacySocket(socket);
  }

  private closeRetiredLegacySocket(socket: WebSocket): void {
    const timer = this.retiredLegacySockets.get(socket);
    if (timer !== undefined) window.clearTimeout(timer);
    this.retiredLegacySockets.delete(socket);
    this.legacySends.delete(socket);
    socket.onopen = null; socket.onmessage = null; socket.onerror = null; socket.onclose = null;
    try { socket.close(4000, "abandoned"); } catch { /* already closing */ }
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
        this.retireLegacySocket(socket);
        this.sockets.delete(session);
        this.clearAttachTimeouts(session);
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
    // pageshow also fires on every load, so only these say the network moved.
    const hinted = () => { this.callbacks.onNetworkChange?.(); this.networkChanged(); };
    const offline = () => { if (this.desired && this.lifecycle.phase === "live") this.connectionLostNow("The network went offline"); };
    // A tab switch is not a network change; coming back after long enough to
    // have slept (or had the radio change) is treated as one.
    const visibility = () => {
      if (document.visibilityState === "hidden") { this.hiddenAt = Date.now(); return; }
      const hiddenFor = this.hiddenAt === undefined ? 0 : Date.now() - this.hiddenAt;
      this.hiddenAt = undefined;
      if (hiddenFor >= HEARTBEAT_INTERVAL_MS) hinted();
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
      hinted();
    };
    window.addEventListener("online", hinted);
    window.addEventListener("offline", offline);
    window.addEventListener("pageshow", changed);
    document.addEventListener("resume", changed);
    document.addEventListener("visibilitychange", visibility);
    connection?.addEventListener?.("change", transportChanged);
    this.removeNetworkListeners = () => {
      window.removeEventListener("online", hinted);
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
    const health = withRequestDeadline(signal => this.probeHealth(signal), undefined, SOCKET_PROBE_TIMEOUT_MS).then(() => true, () => false);
    if (!this.desired) return;
    const socket = this.machineSocket;
    if (this.machineSocketReady && socket?.readyState === WebSocket.OPEN) {
      // Well above any heartbeat ping id (Date.now()), and still an unsigned
      // integer, as the hub's `ping: Option<u64>` requires.
      const id = Date.now() * 10 + 1;
      this.probePingId = id;
      socket.send(JSON.stringify({ channel: "health", ping: id }));
      if (this.probeTimer !== undefined) window.clearTimeout(this.probeTimer);
      this.probeTimer = window.setTimeout(async () => {
        this.probeTimer = undefined;
        if (this.machineSocket === socket && this.probePingId === id) {
          const healthy = await health;
          if (!this.desired || this.machineSocket !== socket || this.probePingId !== id) return;
          this.probePingId = undefined;
          if (healthy) this.reattachDesired("Machine terminal transport stopped answering", true);
          else this.connectionLostNow("The socket and health endpoint stopped answering");
        }
      }, SOCKET_PROBE_TIMEOUT_MS);
      return;
    }
    if (!await health) {
      if (this.desired) this.connectionLostNow("The health endpoint stopped answering");
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

  /** A durable end receipt stops retries; transport closure alone does not. */
  setSessionEnded(session: string, ended: boolean): void {
    if (!ended) { this.endedSessions.delete(session); return; }
    this.endedSessions.add(session);
    this.desiredSessions.delete(session);
    const timer = this.attachRetryTimers.get(session);
    if (timer !== undefined) window.clearTimeout(timer);
    this.attachRetryTimers.delete(session);
    this.clearAttachTimeouts(session);
    const socket = this.sockets.get(session);
    if (socket) this.retireLegacySocket(socket);
    this.sockets.delete(session);
  }

  async attach(session: string): Promise<void> {
    if (this.endedSessions.has(session)) return;
    this.attachesInFlight.set(session, (this.attachesInFlight.get(session) ?? 0) + 1);
    try {
      await this.openAttach(session);
    } catch (error) {
      await this.handleAttachFailure(session, error);
    } finally {
      const left = (this.attachesInFlight.get(session) ?? 1) - 1;
      if (left > 0) this.attachesInFlight.set(session, left);
      else this.attachesInFlight.delete(session);
    }
  }

  /**
   * cas-5dbc: on a live machine, every desired session either is served or
   * has its next attempt coming: a socket opening or open, an attach in
   * flight, or a retry timer. One left with none (an attempt that ended
   * without a socket or a retry, such as an abandon while its ticket was in
   * flight) is scheduled again rather than left "Reconnecting" for good.
   */
  private ensureAttachesPending(): void {
    if (!this.desired || this.lifecycle.phase !== "live" || this.machineSocketOpening) return;
    for (const session of this.desiredSessions) {
      if (this.servedByMachineSocket(session) || this.attachRetryTimers.has(session) || this.attachesInFlight.has(session)) continue;
      // An unsupported browser needs an upgrade, not another attempt.
      if (this.attachLifecycles.get(session)?.fatal) continue;
      const socket = this.sockets.get(session);
      if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) continue;
      this.scheduleAttach(session);
    }
  }

  private async openAttach(session: string): Promise<void> {
    if (!this.desired || this.endedSessions.has(session)) return;
    const unsupported = unsupportedBrowserReason();
    if (unsupported) throw new UnsupportedBrowserError(unsupported);
    this.desiredSessions.add(session);
    if (this.lifecycle.phase !== "live") return;
    if (this.servedByMachineSocket(session)) return;
    const retryTimer = this.attachRetryTimers.get(session);
    if (retryTimer !== undefined) {
      window.clearTimeout(retryTimer);
      this.attachRetryTimers.delete(session);
    }
    if (this.machineMultiplex && !this.machineProtocolBlocked) {
      const connected = await this.ensureMachineSocket(session);
      if (connected) {
        if (!await this.waitForAttachSlot()) return;
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
    if (!this.desired || this.lifecycle.phase !== "live" || !this.desiredSessions.has(session) || generation !== this.machineSocketGeneration) return;
    // A heartbeat/recovery or another caller can attach while the ticket
    // awaits HTTP. Never replace its live/connecting socket: receipts for
    // sends on that socket would fail the current-socket check below.
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
      if (this.sockets.get(session) === socket) {
        this.legacyReadAt.set(socket, Date.now());
        void this.handleDaemonMessage(session, message.data, frameFence);
      } else void this.handleRetiredLegacyReceipt(session, socket, message.data, frameFence);
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
    if (!this.desired || this.lifecycle.phase !== "live" || generation !== this.machineSocketGeneration) return true;
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
    // The session may have ended while this attach waited for its rate slot.
    if (this.endedSessions.has(session) || !this.desiredSessions.has(session)) return;
    const socket = this.machineSocket;
    if (!this.machineSocketReady || !socket || socket.readyState !== WebSocket.OPEN) return;
    if (this.machineSubscriptions.has(session)) return;
    this.machineSubscriptions.add(session);
    this.transitionAttach(session, "attaching", "attaching");
    // Supervisors only (cas-6261): the hub never streams worker panes to Commander.
    socket.send(JSON.stringify({ channel: `pty:${session}`, subscribe: true }));
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
    this.resetAttachRate();
    this.clearAttachTimeouts();
    this.machineSocket?.close(1000, "authentication blocked");
    this.machineSocket = undefined;
    this.machineSocketReady = false;
    this.machineSubscriptions.clear();
    this.healthPing = undefined;
    for (const socket of this.sockets.values()) socket.close(1000, "authentication blocked");
    this.sockets.clear();
    for (const socket of this.retiredLegacySockets.keys()) this.closeRetiredLegacySocket(socket);
    const cause = this.failureCause(new AuthenticationError(kind, detail));
    if (session) this.transitionAttach(session, "failed", "auth", { reason: detail, authFailure: kind, cause });
    this.transition("failed", "auth", { reason: detail, authFailure: kind, cause });
    this.callbacks.onAuthFailure?.(kind, detail);
    if (session) this.callbacks.onSocketError(session, "authentication blocked; re-pair to reconnect");
  }

  private scheduleAttach(session: string): void {
    if (this.endedSessions.has(session)) return;
    if (!this.desired || this.lifecycle.phase !== "live" || this.attachRetryTimers.has(session)) return;
    const attempt = this.socketAttempts.get(session) ?? 0;
    // Capped independently of the machine connection's retry: a session held
    // down for a while must not wait 16–30 s to notice its machine is back.
    // HUB-J11 promises attempts at most 10 s apart.
    const delay = Math.min(SESSION_RETRY_CEILING_MS, backoffDelay(attempt));
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
    this.clearAttachRetryTimers();
    this.socketAttempts.clear();
  }

  private clearAttachRetryTimers(): void {
    for (const timer of this.attachRetryTimers.values()) window.clearTimeout(timer);
    this.attachRetryTimers.clear();
  }

  private resetAttachRate(): void {
    this.attachRateAbort.abort();
    this.attachRateAbort = new AbortController();
    this.attachStarts = [];
    this.attachRateQueue = Promise.resolve();
  }

  private async waitForAttachSlot(parent?: AbortSignal | null): Promise<boolean> {
    const signal = anySignal([this.attachRateAbort.signal, ...(parent ? [parent] : [])]);
    const previous = this.attachRateQueue;
    let release!: () => void;
    this.attachRateQueue = new Promise<void>(resolve => { release = resolve; });
    try {
      await previous;
      if (signal.aborted) return false;
      const wait = (this.attachStarts.at(-2) ?? -Infinity) + 1_000 - Date.now();
      if (wait > 0) await new Promise<void>(resolve => {
        const done = () => { window.clearTimeout(timer); signal.removeEventListener("abort", done); resolve(); };
        const timer = window.setTimeout(done, wait);
        signal.addEventListener("abort", done, { once: true });
      });
      if (signal.aborted || !this.desired || this.lifecycle.phase !== "live") return false;
      this.attachStarts.push(Date.now());
      this.attachStarts = this.attachStarts.slice(-2);
      return true;
    } finally { release(); }
  }

  private startOpenTimeout(session: string, socket: WebSocket): void {
    this.clearAttachTimeouts(session);
    const timeouts = this.attachTimeouts.get(session) ?? {};
    timeouts.open = window.setTimeout(() => {
      if (this.sockets.get(session) !== socket || socket.readyState !== WebSocket.CONNECTING) return;
      this.timedOutSockets.add(socket);
      this.transitionAttach(session, "failed", "dialing", { reason: "Stuck dialing terminal — node may be offline (5s)" });
      this.callbacks.onSocketError(session, "Stuck dialing terminal — node may be offline (5s). Retrying…");
      this.replaceTimedOutSocket(session, socket);
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
      this.replaceTimedOutSocket(session, socket);
    }, STAGE_TIMEOUT_MS.attaching);
    this.attachTimeouts.set(session, timeouts);
  }

  /**
   * cas-5dbc: a timed-out socket is retired and the next attempt scheduled
   * now. Its close() may never finish (a half-open socket), and the attach
   * chain used to wait for that onclose before retrying: the session then
   * stayed "Reconnecting" with no further attempt.
   */
  private replaceTimedOutSocket(session: string, socket: WebSocket): void {
    this.clearAttachTimeouts(session);
    if (this.sockets.get(session) === socket) this.sockets.delete(session);
    this.retireLegacySocket(socket);
    if (this.desired) this.scheduleAttach(session);
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
    if (this.endedSessions.has(session)) return false;
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
    const sent = sendMessageIdentity(outbound);
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
    const index = written.findIndex(send => send.clientRef === clientRef);
    if (!socket || index < 0) return [];
    this.legacySends.set(socket, written.slice(0, index));
    return written.slice(index + 1).map(send => send.clientRef);
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

  /**
   * The pane's attach metadata, when its daemon announced it: the generation
   * and row range Raw output pages through (cas-f285). Undefined for a daemon
   * that predates paged rows.
   */
  paneBootstrap(session: string, paneId: string): PaneBootstrap | undefined {
    return this.paneBootstraps.get(session)?.find((pane) => pane.pane_id === paneId);
  }

  /** Request a bounded page of the pane's rendered rows (cas-f285); answered by onScrollbackPage. */
  requestScrollback(session: string, paneId: string, generation: number, startRow: number, count: number): boolean {
    return this.send(session, { ScrollbackRequest: { pane_id: paneId, generation, start_row: startRow, count } });
  }

  /**
   * Whether the session's daemon serves work events, and whether it pushes
   * change notices and per-agent timelines (cas-ff22).
   */
  workEvents(session: string): WorkEventSupport {
    return this.workEventSupport.get(session) ?? "none";
  }

  /** Request one page of work events: the newest, or the page before `before`. Returns its request id. */
  requestWorkEvents(session: string, options: { before?: string; agent?: string; limit?: number } = {}): string | undefined {
    if (this.workEvents(session) === "none") return undefined;
    const request_id = `work-${crypto.randomUUID()}`;
    const sent = this.send(session, {
      WorkEventsRequest: {
        request_id,
        ...(options.before === undefined ? {} : { before: options.before }),
        limit: Math.min(100, Math.max(1, options.limit ?? 100)),
        ...(options.agent === undefined ? {} : { agent: options.agent }),
      },
    });
    if (!sent) return undefined;
    this.workEventRequests.add(request_id);
    return request_id;
  }

  /** Request the redacted detail of one work event. Returns its request id. */
  requestWorkEventDetail(session: string, eventId: string): string | undefined {
    if (this.workEvents(session) === "none") return undefined;
    const request_id = `work-${crypto.randomUUID()}`;
    if (!this.send(session, { WorkEventDetailRequest: { request_id, event_id: eventId } })) return undefined;
    this.workEventRequests.add(request_id);
    return request_id;
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
      // Kind 2 is a pane keyframe: Commander never asks for one (cas-f285).
      if (kind === 1) this.callbacks.onOutput(session, pane, payload);
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
      // Output was dropped under backpressure. Commander keeps no terminal
      // state to rebuild; Raw output re-reads its rows (cas-f285).
      this.callbacks.onFlowControlReset?.(session);
      return;
    }
    // A replacement may not be in the catalog when this session re-subscribes.
    // These subscribe refusals establish no stream, so release the local claim
    // just as Closed does; the next attempt must actually write a subscription.
    if (envelope.closed || envelope.error?.code === "session_not_found" || envelope.error?.code === "daemon_offline") {
      this.clearAttachTimeouts(session);
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

  /** Retirement cannot revoke positive evidence for a send this socket wrote. */
  private async handleRetiredLegacyReceipt(session: string, socket: WebSocket, input: string | ArrayBuffer | Blob, frameFence: CredentialFence): Promise<void> {
    try {
      const text = typeof input === "string" ? input : input instanceof Blob ? await input.text() : new TextDecoder().decode(input);
      const queued = messageQueuedFromDaemon(JSON.parse(text));
      const current = credentialFence(this.machine);
      const written = this.legacySends.get(socket) ?? [];
      if (!this.retiredLegacySockets.has(socket) || !this.desired || !this.desiredSessions.has(session) || current.credentialId !== frameFence.credentialId || current.generation !== frameFence.generation
        || !queued?.client_ref || !written.some(send => send.clientRef === queued.client_ref && send.target === queued.target)) return;
      // Consume every attempt with this ref: explicit Retry keeps the ref,
      // and one positive receipt settles them all. Duplicate retired frames
      // must not override the first queue identity or keep a handler alive.
      const remaining = written.filter(send => send.clientRef !== queued.client_ref);
      this.legacySends.set(socket, remaining);
      if (!remaining.length) this.closeRetiredLegacySocket(socket);
      this.callbacks.onMessageQueued?.(session, queued, frameFence);
    } catch { /* Malformed/unreadable retired frames provide no receipt. */ }
  }

  private async handleDaemonMessage(session: string, input: string | ArrayBuffer | Blob, frameFence = credentialFence(this.machine)): Promise<void> {
    const text = typeof input === "string" ? input : input instanceof Blob ? await input.text() : new TextDecoder().decode(input);
    const message = JSON.parse(text) as Record<string, any>;
    this.handleDaemonObject(session, message, frameFence);
  }

  private handleDaemonObject(session: string, message: Record<string, any>, frameFence = credentialFence(this.machine)): void {
    if (message.Welcome) {
      // Before the attach turns live: its listeners read what the daemon offers.
      this.workEventSupport.set(session, workEventSupport(message.Welcome.capabilities));
      this.callbacks.onSupervisorCommandSupport?.(session, Array.isArray(message.Welcome.capabilities) && message.Welcome.capabilities.includes("supervisor_commands"));
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
      // cas-f285: no terminal emulator runs here, so no keyframe is asked
      // for. A daemon that announces its panes' rows is read through them;
      // an older one's raw bytes are the only text it offers.
      const bootstrap = Array.isArray(welcome.pane_bootstrap) ? welcome.pane_bootstrap as PaneBootstrap[] : [];
      if (bootstrap.length > 0) this.paneBootstraps.set(session, bootstrap); else this.paneBootstraps.delete(session);
      this.callbacks.onSessionState(session, welcome.state, bootstrap.length > 0 || authoritative ? undefined : welcome.scrollback);
    } else if (message.SupervisorCommandReceipt) {
      this.callbacks.onSupervisorCommandReceipt?.(session, message.SupervisorCommandReceipt.receipt as CommandReceipt);
    } else if (message.ScrollbackPage) {
      this.callbacks.onScrollbackPage?.(session, message.ScrollbackPage as ScrollbackPage);
    } else if (message.PaneKeyframe || message.PaneSize) {
      // Terminal geometry and keyframes are for terminal clients (cas-f285).
    } else if (message.StateUpdate) {
      this.callbacks.onSessionState(session, message.StateUpdate.state);
    } else if (message.Output) {
      this.callbacks.onOutput(session, message.Output.pane_id, new Uint8Array(message.Output.data));
    } else if (message.MessageQueued) {
      const queued = messageQueuedFromDaemon(message);
      // A send reached the daemon: the upstream is back, so the next
      // retryable refusal starts the backoff afresh (cas-a355).
      if (queued) {
        this.clearUpstreamStreak(session);
        const socket = this.sockets.get(session);
        if (socket) this.legacySends.set(socket, (this.legacySends.get(socket) ?? []).filter(send => send.clientRef !== queued.client_ref || send.target !== queued.target));
        this.callbacks.onMessageQueued?.(session, queued, frameFence);
      }
    } else if (message.OperatorReply) {
      this.callbacks.onOperatorReply?.(session, message.OperatorReply as OperatorReply, frameFence);
    } else if (message.OperatorNoticeResolved) {
      this.callbacks.onOperatorNoticeResolved?.(session, message.OperatorNoticeResolved as OperatorNoticeResolved);
    } else if (message.OperatorMessage) {
      this.callbacks.onOperatorMessage?.(session, message.OperatorMessage as ConversationHistoryMessage);
    } else if (message.ConversationHistory) {
      this.callbacks.onConversationHistory?.(session, message.ConversationHistory as ConversationHistoryPage, frameFence);
    } else if (message.WorkEvents) {
      const page = workEventsPage(message.WorkEvents);
      if (page && this.workEventRequests.delete(page.request_id)) this.callbacks.onWorkEvents?.(session, page);
    } else if (message.WorkEventDetail) {
      const detail = workEventDetail(message.WorkEventDetail);
      if (detail && this.workEventRequests.delete(detail.request_id)) this.callbacks.onWorkEventDetail?.(session, detail);
    } else if (message.WorkEventsAppended) {
      this.callbacks.onWorkEventsAppended?.(session);
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
      if (typeof message.Error.client_ref === "string" && this.workEventRequests.delete(message.Error.client_ref)) this.callbacks.onWorkEventsFailed?.(session, message.Error.client_ref, String(message.Error.message ?? "Work events failed"));
      else if (typeof message.Error.client_ref === "string") this.callbacks.onMessageRejected?.(session, message.Error.client_ref, message.Error.message);
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
    ...(typeof value.operator_label === "string" ? { operator_label: value.operator_label } : {}),
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

/** Immutable receipt identity for an outbound SendMessage. */
function sendMessageIdentity(message: unknown): { clientRef: string; target: string } | undefined {
  if (typeof message !== "object" || message === null) return undefined;
  const send = (message as Record<string, unknown>).SendMessage;
  if (!send || typeof send !== "object") return undefined;
  const { client_ref, target } = send as Record<string, unknown>;
  return typeof client_ref === "string" && typeof target === "string" ? { clientRef: client_ref, target } : undefined;
}

function withClientRef(message: unknown, clientRef: string | undefined): unknown {
  if (!clientRef || typeof message !== "object" || message === null) return message;
  const envelope = message as Record<string, unknown>;
  const send = envelope.SendMessage;
  if (!send || typeof send !== "object") return message;
  return { ...envelope, SendMessage: { ...(send as Record<string, unknown>), client_ref: clientRef } };
}
