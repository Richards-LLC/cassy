import { dpopHeaders } from "./dpop";
import type { HubSession, LeaseState, PaneInfo, SessionState, StoredMachine } from "./types";

export type ConnectionState = "idle" | "connecting" | "connected" | "reconnecting" | "auth-blocked" | "offline";

export interface HubCallbacks {
  onState(state: ConnectionState, detail?: string): void;
  onSessions(sessions: HubSession[]): void;
  onMachineEvent(event: Record<string, unknown>): void;
  onSessionState(session: string, state: SessionState, scrollback?: Record<string, number[][]>): void;
  onOutput(session: string, paneId: string, data: Uint8Array): void;
  onSocketError(session: string, detail: string): void;
}

const RETRY_DELAYS = [1_000, 2_000, 4_000, 8_000, 16_000];

class AuthenticationError extends Error {}

export class HubConnectionSupervisor {
  private desired = false;
  private attempt = 0;
  private eventAbort?: AbortController;
  private retryTimer?: number;
  private readonly sockets = new Map<string, WebSocket>();
  private readonly socketAttempts = new Map<string, number>();

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
    for (const socket of this.sockets.values()) socket.close(1000, "machine removed");
    this.sockets.clear();
    this.callbacks.onState("idle");
  }

  private async connect(): Promise<void> {
    if (!this.desired) return;
    this.callbacks.onState(this.attempt === 0 ? "connecting" : "reconnecting");
    try {
      await this.refreshSessions();
      this.attempt = 0;
      this.callbacks.onState("connected");
      await this.streamEvents();
      if (this.desired) throw new Error("hub event stream closed");
    } catch (error) {
      if (!this.desired || error instanceof DOMException && error.name === "AbortError") return;
      if (error instanceof AuthenticationError) {
        this.blockAuthentication(error.message);
        return;
      }
      if (!navigator.onLine) this.callbacks.onState("offline", "browser offline");
      const delay = RETRY_DELAYS[Math.min(this.attempt, RETRY_DELAYS.length - 1)];
      this.attempt += 1;
      this.callbacks.onState("reconnecting", `retrying in ${delay / 1000}s`);
      this.retryTimer = window.setTimeout(() => void this.connect(), delay);
    }
  }

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers = await dpopHeaders(this.machine, method, path);
    const response = await fetch(new URL(path, this.machine.baseUrl), {
      method,
      headers: { ...headers, ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
      body: body === undefined ? undefined : JSON.stringify(body),
      cache: "no-store",
      credentials: "omit",
    });
    if (response.status === 401 || response.status === 403) throw new AuthenticationError("pairing expired or was revoked");
    if (!response.ok) throw new Error(`${method} ${path} failed (${response.status})`);
    if (response.status === 204) return undefined as T;
    return response.json() as Promise<T>;
  }

  async refreshSessions(): Promise<HubSession[]> {
    const response = await this.request<{ sessions: HubSession[] }>("GET", "/v1/sessions");
    this.callbacks.onSessions(response.sessions);
    return response.sessions;
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

  async releaseLease(session: string): Promise<void> {
    await this.request("DELETE", `/v1/sessions/${encodeURIComponent(session)}/lease`);
  }

  private async streamEvents(): Promise<void> {
    this.eventAbort = new AbortController();
    const path = "/v1/events";
    const response = await fetch(new URL(path, this.machine.baseUrl), {
      headers: await dpopHeaders(this.machine, "GET", path),
      signal: this.eventAbort.signal,
      cache: "no-store",
      credentials: "omit",
    });
    if (response.status === 401 || response.status === 403) throw new AuthenticationError("pairing expired or was revoked");
    if (!response.ok || !response.body) throw new Error(`event stream failed (${response.status})`);
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

  async attach(session: string): Promise<void> {
    const existing = this.sockets.get(session);
    if (existing && (existing.readyState === WebSocket.OPEN || existing.readyState === WebSocket.CONNECTING)) return;
    const ticket = await this.request<{ ticket: string }>("POST", "/v1/auth/websocket-ticket", { session });
    const endpoint = new URL(`/v1/sessions/${encodeURIComponent(session)}/attach`, this.machine.baseUrl);
    endpoint.protocol = endpoint.protocol === "https:" ? "wss:" : "ws:";
    endpoint.searchParams.set("ticket", ticket.ticket);
    const socket = new WebSocket(endpoint);
    socket.binaryType = "arraybuffer";
    this.sockets.set(session, socket);
    socket.onopen = () => this.socketAttempts.set(session, 0);
    socket.onmessage = (message) => this.handleDaemonMessage(session, message.data);
    socket.onclose = (event) => {
      if (this.sockets.get(session) === socket) this.sockets.delete(session);
      if (!this.desired || event.code === 1000) return;
      this.scheduleAttach(session);
    };
    socket.onerror = () => this.callbacks.onSocketError(session, "terminal transport error");
  }

  private async handleAttachFailure(session: string, error: unknown): Promise<void> {
    if (error instanceof AuthenticationError) {
      this.blockAuthentication(error.message, session);
      return;
    }
    // A revoked origin is rejected at CORS preflight before the authenticated
    // request can expose its 401/403 status. Distinguish that terminal policy
    // refusal from an offline hub with a credential-free opaque health probe.
    if (await this.hubIsReachable()) {
      this.blockAuthentication("pairing expired or was revoked", session);
      return;
    }
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

  private blockAuthentication(detail: string, session?: string): void {
    this.desired = false;
    this.eventAbort?.abort();
    if (this.retryTimer !== undefined) window.clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    for (const socket of this.sockets.values()) socket.close(1000, "authentication blocked");
    this.sockets.clear();
    this.callbacks.onState("auth-blocked", detail);
    if (session) this.callbacks.onSocketError(session, "authentication blocked; re-pair to reconnect");
  }

  private scheduleAttach(session: string): void {
    if (!this.desired) return;
    const attempt = this.socketAttempts.get(session) ?? 0;
    const delay = RETRY_DELAYS[Math.min(attempt, RETRY_DELAYS.length - 1)];
    this.socketAttempts.set(session, attempt + 1);
    window.setTimeout(() => void this.attach(session).catch((error) => this.handleAttachFailure(session, error)), delay);
  }

  send(session: string, message: unknown): boolean {
    const socket = this.sockets.get(session);
    if (!socket || socket.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(message));
    return true;
  }

  private async handleDaemonMessage(session: string, input: string | ArrayBuffer | Blob): Promise<void> {
    const text = typeof input === "string" ? input : input instanceof Blob ? await input.text() : new TextDecoder().decode(input);
    const message = JSON.parse(text) as Record<string, any>;
    if (message.Welcome) {
      this.callbacks.onSessionState(session, message.Welcome.state, message.Welcome.scrollback);
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
