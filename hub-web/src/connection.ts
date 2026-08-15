import { dpopHeaders } from "./dpop";
import {
  backoffDelay,
  DEGRADED_AFTER_MISSED_HEARTBEATS,
  HEARTBEAT_INTERVAL_MS,
  RECONNECT_AFTER_MISSED_HEARTBEATS,
  stageFailureDetail,
  STAGE_TIMEOUT_MS,
  type ConnectionPhase,
  type ConnectionSnapshot,
  type ConnectionStage,
  type AttachSnapshot,
} from "./connection-state";
import type { HubSession, LeaseState, PaneInfo, SessionState, StoredMachine } from "./types";

export type ConnectionState = ConnectionSnapshot;
export type AuthFailureKind = "expired" | "revoked" | "scope-mismatch";

export interface HubMachineInfo {
  schema_version: number;
  version: string;
  capabilities: string[];
}

export interface HubCallbacks {
  onState(state: ConnectionState): void;
  onAttachState?(session: string, state: AttachSnapshot): void;
  onLatency?(latencyMs: number): void;
  onAuthFailure?(kind: AuthFailureKind, detail: string): void;
  onCredentialRefreshed?(machine: StoredMachine): Promise<void> | void;
  onMachineInfo?(machine: HubMachineInfo | undefined): void;
  onSessions(sessions: HubSession[]): void;
  onMachineEvent(event: Record<string, unknown>): void;
  onSessionState(session: string, state: SessionState, scrollback?: Record<string, number[][]>, authoritativeKeyframes?: boolean): void;
  onOutput(session: string, paneId: string, data: Uint8Array): void;
  onPaneKeyframe(session: string, paneId: string, data: Uint8Array): void;
  onScrollbackPage?(session: string, page: Record<string, any>): void;
  onSocketError(session: string, detail: string): void;
}

class AuthenticationError extends Error {
  constructor(readonly kind: AuthFailureKind, message: string) { super(message); }
}

export class HubConnectionSupervisor {
  private desired = false;
  private attempt = 0;
  private eventAbort?: AbortController;
  private retryTimer?: number;
  private heartbeatTimer?: number;
  private missedHeartbeats = 0;
  private lastHeartbeatAt?: number;
  private expiredRefreshAttempted = false;
  private resumeStage: ConnectionStage = "resolving";
  private lifecycle: ConnectionSnapshot = {
    phase: "idle", stage: "idle", since: Date.now(), attempt: 0, missedHeartbeats: 0, degraded: false,
  };
  private readonly sockets = new Map<string, WebSocket>();
  private readonly keyframeRequests = new Set<string>();
  private readonly attachLifecycles = new Map<string, AttachSnapshot>();
  private readonly socketAttempts = new Map<string, number>();
  private readonly attachRetryTimers = new Map<string, number>();
  private readonly attachTimeouts = new Map<string, { open?: number; ready?: number }>();
  private readonly timedOutSockets = new WeakSet<WebSocket>();
  private readonly readySockets = new WeakSet<WebSocket>();

  constructor(readonly machine: StoredMachine, private readonly callbacks: HubCallbacks) {}

  start(): void {
    if (this.desired) return;
    this.desired = true;
    void this.connect();
  }

  stop(): void {
    this.desired = false;
    this.eventAbort?.abort();
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    if (this.heartbeatTimer !== undefined) window.clearInterval(this.heartbeatTimer);
    this.heartbeatTimer = undefined;
    this.clearAttachRetries();
    this.clearAttachTimeouts();
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
    this.lifecycle = {
      phase,
      stage,
      since: Date.now(),
      attempt: this.attempt,
      missedHeartbeats: this.missedHeartbeats,
      degraded: this.missedHeartbeats >= DEGRADED_AFTER_MISSED_HEARTBEATS,
      ...update,
    };
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
      ...update,
    };
    this.attachLifecycles.set(session, snapshot);
    this.callbacks.onAttachState?.(session, snapshot);
  }

  private async connect(): Promise<void> {
    if (!this.desired) return;
    let stage = this.resumeStage;
    try {
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
      this.attempt = 0;
      this.resumeStage = "resolving";
      this.missedHeartbeats = 0;
      this.lastHeartbeatAt = Date.now();
      this.transition("live", "live", { latencyMs: 0 });
      this.startHeartbeat();
      await this.consumeEvents(response);
      if (this.desired) throw new Error("hub event stream closed");
    } catch (error) {
      if (!this.desired) return;
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
          }
        }
        this.blockAuthentication(authError.kind, authError.message);
        return;
      }
      this.stopHeartbeat();
      this.resumeStage = stage;
      const reason = error instanceof Error ? error.message : "unknown connection failure";
      const target = new URL(this.machine.baseUrl).host;
      this.transition("failed", stage, { reason: stageFailureDetail(stage, target, reason) });
      const delay = backoffDelay(this.attempt++);
      this.transition("backoff", stage, { reason: stageFailureDetail(stage, target, reason), retryInMs: delay });
      this.retryTimer = window.setTimeout(() => {
        this.retryTimer = undefined;
        if (this.desired) void this.connect();
      }, delay);
    }
  }

  private async withStageTimeout<T>(stage: Exclude<ConnectionStage, "idle" | "live">, task: (signal: AbortSignal) => Promise<T>): Promise<T> {
    const controller = new AbortController();
    const timer = window.setTimeout(() => controller.abort(new DOMException(`${stage} timed out after ${STAGE_TIMEOUT_MS[stage] / 1000}s`, "TimeoutError")), STAGE_TIMEOUT_MS[stage]);
    try { return await task(controller.signal); }
    catch (error) {
      if (controller.signal.aborted) throw controller.signal.reason;
      throw error;
    } finally { window.clearTimeout(timer); }
  }

  private async probeHealth(signal: AbortSignal): Promise<void> {
    const response = await fetch(new URL("/v1/health", this.machine.baseUrl), { signal, cache: "no-store", credentials: "omit" });
    if (!response.ok) throw new Error(`daemon health failed (${response.status})`);
  }

  async request<T>(method: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
    const startedAt = performance.now();
    const headers = await dpopHeaders(this.machine, method, path);
    const response = await fetch(new URL(path, this.machine.baseUrl), {
      method,
      headers: { ...headers, ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
      body: body === undefined ? undefined : JSON.stringify(body),
      cache: "no-store",
      credentials: "omit",
      signal,
    });
    if (response.status === 401 || response.status === 403) {
      const kind: AuthFailureKind = Date.parse(this.machine.expiresAt) <= Date.now()
        ? "expired" : response.status === 403 ? "scope-mismatch" : "revoked";
      throw new AuthenticationError(kind, kind === "expired" ? "pairing expired" : kind === "revoked" ? "pairing was revoked" : "credential ceiling does not grant this operation");
    }
    if (!response.ok) throw new Error(`${method} ${path} failed (${response.status})`);
    this.callbacks.onLatency?.(Math.max(0, Math.round(performance.now() - startedAt)));
    if (response.status === 204) return undefined as T;
    return response.json() as Promise<T>;
  }

  async refreshSessions(signal?: AbortSignal): Promise<HubSession[]> {
    const response = await this.request<{ sessions: HubSession[] }>("GET", "/v1/sessions", undefined, signal);
    this.callbacks.onSessions(response.sessions);
    return response.sessions;
  }

  private async refreshMachineInfo(signal?: AbortSignal): Promise<void> {
    try {
      this.callbacks.onMachineInfo?.(await this.request<HubMachineInfo>("GET", "/v1/machine", undefined, signal));
    } catch (error) {
      if (error instanceof AuthenticationError) throw error;
      // Older hubs can still offer the read-only session surface. The UI shows
      // a visible compatibility warning and leaves capability-gated controls off.
      this.callbacks.onMachineInfo?.(undefined);
    }
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

  async releaseLease(session: string): Promise<void> {
    await this.request("DELETE", `/v1/sessions/${encodeURIComponent(session)}/lease`);
  }

  async diagnose(): Promise<Record<string, unknown>> {
    const report = await this.request<Record<string, unknown>>("GET", "/v1/diagnostics");
    return { ...report, browser: { online: navigator.onLine, last_successful_heartbeat: this.lastHeartbeatAt ? new Date(this.lastHeartbeatAt).toISOString() : null } };
  }

  private async openEventStream(signal: AbortSignal): Promise<Response> {
    this.eventAbort = new AbortController();
    const path = "/v1/events";
    const response = await fetch(new URL(path, this.machine.baseUrl), {
      headers: await dpopHeaders(this.machine, "GET", path),
      signal: AbortSignal.any([this.eventAbort.signal, signal]),
      cache: "no-store",
      credentials: "omit",
    });
    if (response.status === 401 || response.status === 403) {
      const kind: AuthFailureKind = Date.parse(this.machine.expiresAt) <= Date.now() ? "expired" : response.status === 403 ? "scope-mismatch" : "revoked";
      throw new AuthenticationError(kind, "event-stream authentication failed");
    }
    if (!response.ok || !response.body) throw new Error(`event stream failed (${response.status})`);
    return response;
  }

  private async consumeEvents(response: Response): Promise<void> {
    if (!response.body) throw new Error("event stream closed before attach");
    const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
    let buffer = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buffer += value;
      let boundary = buffer.indexOf("\n\n");
      while (boundary >= 0) {
        const block = buffer.slice(0, boundary);
        buffer = buffer.slice(boundary + 2);
        const data = block.split("\n").filter((line) => line.startsWith("data:")).map((line) => line.slice(5).trim()).join("\n");
        if (data) {
          const event = JSON.parse(data) as Record<string, unknown>;
          this.callbacks.onMachineEvent(event);
          await this.refreshSessions();
        }
        boundary = buffer.indexOf("\n\n");
      }
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
    const started = performance.now();
    try {
      await this.request("GET", "/v1/machine", undefined, AbortSignal.timeout(3_000));
      this.missedHeartbeats = 0;
      this.lastHeartbeatAt = Date.now();
      this.transition("live", "live", { latencyMs: Math.round(performance.now() - started) });
    } catch (error) {
      this.missedHeartbeats += 1;
      this.transition("live", "live", { reason: error instanceof Error ? error.message : "heartbeat failed" });
      if (this.missedHeartbeats >= RECONNECT_AFTER_MISSED_HEARTBEATS) {
        this.stopHeartbeat();
        this.eventAbort?.abort();
        this.resumeStage = "dialing";
      }
    }
  }

  private async refreshCredential(): Promise<void> {
    const refreshed = await this.request<{ credential: string; credential_id: string; expires_at: string; scopes: StoredMachine["scopes"] }>("POST", "/v1/auth/refresh");
    this.machine.credential = refreshed.credential;
    this.machine.credentialId = refreshed.credential_id;
    this.machine.expiresAt = refreshed.expires_at;
    this.machine.scopes = refreshed.scopes;
    this.expiredRefreshAttempted = false;
    await this.callbacks.onCredentialRefreshed?.(this.machine);
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
    const retryTimer = this.attachRetryTimers.get(session);
    if (retryTimer !== undefined) {
      window.clearTimeout(retryTimer);
      this.attachRetryTimers.delete(session);
    }
    const existing = this.sockets.get(session);
    if (existing && (existing.readyState === WebSocket.OPEN || existing.readyState === WebSocket.CONNECTING)) return;
    this.transitionAttach(session, "auth", "auth");
    const ticket = await this.request<{ ticket: string }>("POST", "/v1/auth/websocket-ticket", { session });
    if (!this.desired) return;
    const endpoint = new URL(`/v1/sessions/${encodeURIComponent(session)}/attach`, this.machine.baseUrl);
    endpoint.protocol = endpoint.protocol === "https:" ? "wss:" : "ws:";
    endpoint.searchParams.set("ticket", ticket.ticket);
    const socket = new WebSocket(endpoint);
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
    socket.onmessage = (message) => this.handleDaemonMessage(session, message.data);
    socket.onclose = (event) => {
      const timedOut = this.timedOutSockets.has(socket);
      const becameReady = this.readySockets.has(socket);
      this.clearAttachTimeouts(session);
      if (this.sockets.get(session) === socket) this.sockets.delete(session);
      if (!this.desired || event.code === 1000) return;
      if (!timedOut) {
        const detail = becameReady ? "Terminal connection closed" : "Terminal connection closed before it became ready";
        this.transitionAttach(session, "failed", becameReady ? "dialing" : (this.attachLifecycles.get(session)?.stage ?? "dialing"), { reason: detail });
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

  private async handleAttachFailure(session: string, error: unknown): Promise<void> {
    if (!this.desired) return;
    if (error instanceof AuthenticationError) {
      this.transitionAttach(session, "failed", "auth", { reason: error.message, authFailure: error.kind });
      this.blockAuthentication(error.kind, error.message, session);
      return;
    }
    // A revoked origin is rejected at CORS preflight before the authenticated
    // request can expose its 401/403 status. Distinguish that terminal policy
    // refusal from an offline hub with a credential-free opaque health probe.
    const reachable = await this.hubIsReachable();
    if (!this.desired) return;
    if (reachable) {
      this.transitionAttach(session, "failed", "auth", { reason: "pairing expired or was revoked", authFailure: "revoked" });
      this.blockAuthentication("revoked", "pairing expired or was revoked", session);
      return;
    }
    const detail = error instanceof Error ? error.message : "unknown terminal attach failure";
    const failedStage = this.attachLifecycles.get(session)?.stage ?? "dialing";
    this.transitionAttach(session, "failed", failedStage, { reason: stageFailureDetail(failedStage, new URL(this.machine.baseUrl).host, detail) });
    this.callbacks.onSocketError(session, `Terminal attach failed: ${detail}. Retrying…`);
    this.scheduleAttach(session);
  }

  private async hubIsReachable(): Promise<boolean> {
    try {
      await fetch(new URL("/v1/health", this.machine.baseUrl), {
        method: "GET",
        mode: "no-cors",
        cache: "no-store",
        credentials: "omit",
        signal: AbortSignal.timeout(3_000),
      });
      return true;
    } catch {
      return false;
    }
  }

  private blockAuthentication(kind: AuthFailureKind, detail: string, session?: string): void {
    this.desired = false;
    this.eventAbort?.abort();
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    this.clearAttachRetries();
    this.clearAttachTimeouts();
    for (const socket of this.sockets.values()) socket.close(1000, "authentication blocked");
    this.sockets.clear();
    if (session) this.transitionAttach(session, "failed", "auth", { reason: detail, authFailure: kind });
    this.transition("failed", "auth", { reason: detail, authFailure: kind });
    this.callbacks.onAuthFailure?.(kind, detail);
    if (session) this.callbacks.onSocketError(session, "authentication blocked; re-pair to reconnect");
  }

  private scheduleAttach(session: string): void {
    if (!this.desired || this.attachRetryTimers.has(session)) return;
    const attempt = this.socketAttempts.get(session) ?? 0;
    const delay = backoffDelay(attempt);
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

  send(session: string, message: unknown): boolean {
    const socket = this.sockets.get(session);
    if (!socket || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  requestPaneKeyframe(session: string, paneId: string): boolean {
    const key = `${session}:${paneId}`;
    if (this.keyframeRequests.has(key)) return true;
    if (!this.send(session, { RequestPaneKeyframe: { pane_id: paneId } })) return false;
    this.keyframeRequests.add(key);
    return true;
  }

  requestScrollback(session: string, paneId: string, generation: number, startRow: number, count = 200): boolean {
    return this.send(session, {
      ScrollbackRequest: {
        pane_id: paneId,
        generation,
        start_row: startRow,
        count: Math.min(200, Math.max(1, count)),
      },
    });
  }

  private async handleDaemonMessage(session: string, input: string | ArrayBuffer | Blob): Promise<void> {
    const text = typeof input === "string" ? input : input instanceof Blob ? await input.text() : new TextDecoder().decode(input);
    const message = JSON.parse(text) as Record<string, any>;
    if (message.Welcome) {
      const socket = this.sockets.get(session);
      if (socket) {
        this.readySockets.add(socket);
        this.clearAttachTimeout(session, "ready");
        this.socketAttempts.set(session, 0);
        this.transitionAttach(session, "live", "live");
      }
      const welcome = message.Welcome;
      const authoritative = Number(welcome.protocol_version ?? 1) >= 3
        && Array.isArray(welcome.capabilities)
        && welcome.capabilities.includes("authoritative_pane_keyframes");
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
    } else if (message.ScrollbackPage) {
      this.callbacks.onScrollbackPage?.(session, message.ScrollbackPage);
    } else if (message.StateUpdate) {
      this.callbacks.onSessionState(session, message.StateUpdate.state);
    } else if (message.Output) {
      this.callbacks.onOutput(session, message.Output.pane_id, new Uint8Array(message.Output.data));
    } else if (message.PaneAdded || message.PaneRemoved || message.PaneExited) {
      this.send(session, "GetState");
    } else if (message.Error) {
      this.callbacks.onSocketError(session, message.Error.message);
    }
  }
}
