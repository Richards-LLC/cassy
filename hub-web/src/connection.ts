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
import type { HubSession, LeaseState, PaneInfo, SessionCardSummary, SessionState, StoredMachine } from "./types";

export type ConnectionState = ConnectionSnapshot;
export type AuthFailureKind = "expired" | "revoked" | "scope-mismatch" | "needs-pairing";

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
  onSessionSummary?(session: string, summary: SessionCardSummary): void;
  onPaneKeyframe(session: string, paneId: string, data: Uint8Array): void;
  onFlowControlReset?(session: string): void;
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
  private machineSocket?: WebSocket;
  private machineSocketReady = false;
  private machineSocketOpening?: Promise<boolean>;
  private machineMultiplex = false;
  private machineProtocolBlocked = false;
  private readonly desiredSessions = new Set<string>();
  private readonly machineSubscriptions = new Set<string>();
  private readonly sessionPanes = new Map<string, PaneInfo[]>();
  private healthPing?: { id: number; startedAt: number };
  private lastMachineEventSequence = 0;

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
      this.transition("live", "live");
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
      // The health probe succeeded immediately before this stage. An opaque
      // browser failure on an authenticated route is how an unpaired origin
      // appears when CORS preflight withholds the response; do not present it
      // as an offline hub or keep retrying an action that needs re-pairing.
      if (stage === "auth") {
        this.blockAuthentication(
          "needs-pairing",
          "Hub is reachable but this Cassy Commander is no longer paired. Re-pair to continue.",
        );
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
      const info = await this.request<HubMachineInfo>("GET", "/v1/machine", undefined, signal);
      this.machineMultiplex = info.capabilities.includes("machine_multiplex_v2");
      this.callbacks.onMachineInfo?.(info);
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
          this.deliverMachineEvent(event);
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
      if (this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN) {
        if (this.healthPing) {
          this.missedHeartbeats += 1;
          this.transition("live", "live", { reason: "machine WebSocket heartbeat missed" });
          if (this.missedHeartbeats >= RECONNECT_AFTER_MISSED_HEARTBEATS) {
            this.machineSocket.close(1012, "heartbeat timeout");
            return;
          }
        }
        const id = Date.now();
        this.healthPing = { id, startedAt: started };
        this.machineSocket.send(JSON.stringify({ channel: "health", ping: id }));
        return;
      }
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

  private async ensureMachineSocket(session: string): Promise<boolean> {
    if (this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN) return true;
    if (this.machineSocketOpening) return this.machineSocketOpening;
    this.machineSocketOpening = this.openMachineSocket(session).finally(() => {
      this.machineSocketOpening = undefined;
    });
    return this.machineSocketOpening;
  }

  private async openMachineSocket(session: string): Promise<boolean> {
    this.transitionAttach(session, "auth", "auth");
    let ticket: { ticket: string };
    try {
      ticket = await this.request<{ ticket: string }>("POST", "/v1/auth/websocket-ticket", {});
    } catch (error) {
      if (error instanceof AuthenticationError) throw error;
      // A hub from before machine protocol v2 advertised no capability, but a
      // rolling upgrade can briefly expose stale machine metadata. Preserve
      // the old per-session attach as a bounded compatibility fallback.
      this.machineMultiplex = false;
      return false;
    }
    if (!this.desired) return true;
    const endpoint = new URL("/v1/attach", this.machine.baseUrl);
    endpoint.protocol = endpoint.protocol === "https:" ? "wss:" : "ws:";
    endpoint.searchParams.set("ticket", ticket.ticket);
    const socket = new WebSocket(endpoint);
    socket.binaryType = "arraybuffer";
    this.machineSocket = socket;
    for (const desired of this.desiredSessions) this.transitionAttach(desired, "dialing", "dialing");

    return new Promise<boolean>((resolve) => {
      let settled = false;
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
        this.machineProtocolBlocked = true;
        clearTimers();
        for (const desired of this.desiredSessions) {
          this.transitionAttach(desired, "failed", "attaching", { reason: detail });
          this.callbacks.onSocketError(desired, detail);
        }
        if (!settled) {
          settled = true;
          resolve(true);
        }
        socket.close(1002, "protocol mismatch");
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
            protocolFailure(`Machine protocol mismatch: Cassy Commander requires proto 2${supported ? `; hub supports ${supported}` : ""}`);
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
        void this.handleMachineMessage(event.data);
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
          this.transitionAttach(desired, "failed", "dialing", { reason: "Machine terminal transport closed" });
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
    socket.send(JSON.stringify({ channel: `pty:${session}`, subscribe: true }));
    const timeouts = this.attachTimeouts.get(session) ?? {};
    if (timeouts.ready !== undefined) window.clearTimeout(timeouts.ready);
    timeouts.ready = window.setTimeout(() => {
      if (!this.machineSocketReady || this.attachLifecycles.get(session)?.phase === "live") return;
      this.transitionAttach(session, "failed", "attaching", { reason: "Machine stream sent no session state within 3s" });
      this.callbacks.onSocketError(session, "Machine stream sent no session state within 3s. Retrying…");
      this.scheduleAttach(session);
    }, STAGE_TIMEOUT_MS.attaching);
    this.attachTimeouts.set(session, timeouts);
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
    this.machineSocket?.close(1000, "authentication blocked");
    this.machineSocket = undefined;
    this.machineSocketReady = false;
    this.machineSubscriptions.clear();
    this.healthPing = undefined;
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
    if (this.machineSocketReady && this.machineSocket?.readyState === WebSocket.OPEN) {
      const resize = typeof message === "object" && message !== null && "ResizePane" in message;
      this.machineSocket.send(JSON.stringify(resize
        ? { channel: "resize", session, message }
        : { channel: `pty:${session}`, message }));
      return true;
    }
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

  private async handleMachineMessage(input: string | ArrayBuffer | Blob): Promise<void> {
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
      if (this.healthPing?.id !== envelope.pong) return;
      const latencyMs = Math.max(0, Math.round(performance.now() - this.healthPing.startedAt));
      this.healthPing = undefined;
      this.missedHeartbeats = 0;
      this.lastHeartbeatAt = Date.now();
      this.transition("live", "live", { latencyMs });
      this.callbacks.onLatency?.(latencyMs);
      return;
    }
    if (envelope.channel === "events" && envelope.event) {
      this.deliverMachineEvent(envelope.event as Record<string, unknown>);
      await this.refreshSessions();
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
      this.transitionAttach(session, "failed", "attaching", { reason: "Session daemon stream closed" });
      this.callbacks.onSocketError(session, "Session daemon stream closed. Retrying…");
      this.scheduleAttach(session);
      return;
    }
    if (envelope.error) {
      const detail = String(envelope.error.message ?? envelope.error.code ?? "machine protocol error");
      this.callbacks.onSocketError(session, detail);
      return;
    }
    if (envelope.message) this.handleDaemonObject(session, envelope.message as Record<string, any>);
  }

  private deliverMachineEvent(event: Record<string, unknown>): void {
    const sequence = Number(event.sequence ?? 0);
    if (Number.isFinite(sequence) && sequence > 0) {
      if (sequence <= this.lastMachineEventSequence) return;
      this.lastMachineEventSequence = sequence;
    }
    this.callbacks.onMachineEvent(event);
  }

  private async handleDaemonMessage(session: string, input: string | ArrayBuffer | Blob): Promise<void> {
    const text = typeof input === "string" ? input : input instanceof Blob ? await input.text() : new TextDecoder().decode(input);
    const message = JSON.parse(text) as Record<string, any>;
    this.handleDaemonObject(session, message);
  }

  private handleDaemonObject(session: string, message: Record<string, any>): void {
    if (message.Welcome) {
      const socket = this.sockets.get(session);
      if (socket) {
        this.readySockets.add(socket);
        this.clearAttachTimeout(session, "ready");
        this.socketAttempts.set(session, 0);
        this.transitionAttach(session, "live", "live");
      } else if (this.machineSocketReady) {
        this.clearAttachTimeout(session, "ready");
        this.socketAttempts.set(session, 0);
        this.transitionAttach(session, "live", "live");
      }
      const welcome = message.Welcome;
      this.sessionPanes.set(session, welcome.state.panes);
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
      this.sessionPanes.set(session, message.StateUpdate.state.panes);
      this.callbacks.onSessionState(session, message.StateUpdate.state);
    } else if (message.Output) {
      this.callbacks.onOutput(session, message.Output.pane_id, new Uint8Array(message.Output.data));
    } else if (message.SessionSummary) {
      this.callbacks.onSessionSummary?.(session, message.SessionSummary.summary);
    } else if (message.PaneAdded || message.PaneRemoved || message.PaneExited) {
      this.send(session, "GetState");
    } else if (message.Error) {
      this.callbacks.onSocketError(session, message.Error.message);
    }
  }
}
