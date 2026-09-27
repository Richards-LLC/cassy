// Hub protocol double for the user-journey suite.
//
// The page under test is the production bundle (hub-web/dist). Everything the
// bundle says to a machine's hub (HTTPS on https://<id>.test, WebSockets) and
// to the pairing relay is answered here, at the network boundary. Payload
// shapes follow the hub wire types in src/types.ts and the relay contract in
// src/pairing-relay.ts. Evidence label: "real-bundle, protocol-double".
import type { Page, Route, WebSocketRoute } from "@playwright/test";

export const RELAY = "https://petra-stella-cloud.vercel.app";
export const SCOPES = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];

export type Session = {
  name: string;
  supervisor: string;
  project_dir: string;
  workers: string[];
  liveness: "live";
  dormant?: boolean;
};

export type Machine = { id: string; label: string; sessions: Session[] };

export type SentMessage = {
  machine: string;
  session: string;
  client_ref: string;
  target: string;
  text: string;
  in_reply_to?: number;
};

export type HistoryPage = {
  messages: Array<Record<string, unknown>>;
  replies: Array<Record<string, unknown>>;
  has_earlier: boolean;
  next_before?: number;
};

export type DoubleOptions = {
  machines: Machine[];
  /** Seed these machine ids as already paired (IndexedDB), as a returning user. */
  paired?: string[];
  /** Conversation history pages served in order for each session, newest first. */
  history?: Record<string, HistoryPage[]>;
  /** Relay pairing: which machine the relay authorizes, and after how many polls. */
  relay?: { machine: string; claimAfter: number; authorizeAfter: number };
  /** How far each session's machine clock runs ahead of this browser (ms): the
   * stamps its replayed history carries, as a skewed daemon's would. */
  clockAheadMs?: Record<string, number>;
  /** Advertise machine protocol v2 and serve /v1/attach as the real hub does
   * (one socket per machine, `pty:<session>` channels, health ping/pong). */
  multiplex?: boolean;
};

/** A machine the browser cannot reach: how the outage looks from the page. */
export type Outage = {
  /** Established sockets stop carrying frames either way without closing, as
   * a half-open TCP connection does after an interface change (default), or
   * close as a reset would. */
  sockets?: "stall" | "close";
};

const PANE_TEXT = "The supervisor is ready.\r\n";
/** The real hub's operator text for `upstream_unavailable` (hub/server.rs). */
const UPSTREAM_UNAVAILABLE_MESSAGE = "The session's daemon connection is reconnecting, so the message was not sent. Retry once the session is live again.";

export class HubDouble {
  readonly sends: SentMessage[] = [];
  readonly exchanges: Array<Record<string, unknown>> = [];
  /** The hub origin each pairing exchange was posted to, in order. */
  readonly exchangeOrigins: string[] = [];
  readonly historyRequests: Array<Record<string, unknown>> = [];
  /** Artifact ids Commander asked a signed view URL for (cassy#910). */
  readonly artifactRequests: string[] = [];
  private readonly sockets = new Map<string, WebSocketRoute>();
  private readonly waiters: Array<() => void> = [];
  private readonly held = new Set<string>();
  private readonly attachDelays = new Map<string, number>();
  private polls = 0;
  private requestedScopes: string[] = [];
  private nextId = 1000;
  /** Machines currently unreachable (network down, Tailscale off), and how. */
  private readonly outages = new Map<string, Outage>();
  /** Every machine socket, by machine, open or stalled. */
  private readonly machineSockets = new Map<string, Set<WebSocketRoute>>();
  /** Session sockets by machine, for outages. */
  private readonly sessionSocketsByMachine = new Map<string, Set<WebSocketRoute>>();
  /** Health pings the double answered, per machine. */
  readonly pongs = new Map<string, number>();
  /** Sockets that were open when their machine went down: half-open for good. */
  private readonly stalledSockets = new WeakSet<WebSocketRoute>();
  /** Machine sockets opened, per machine (multiplex). */
  readonly machineSocketOpens = new Map<string, number>();
  /** Legacy per-session sockets opened, per session (cas-2036). */
  readonly legacySocketOpens = new Map<string, number>();
  /** Sessions whose daemon upstream is gone while the hub stays up (cas-0653). */
  private readonly upstreamDown = new Set<string>();
  /** How long a legacy-socket refusal takes to reach the page, per session (cas-2036). */
  private readonly upstreamRefusalDelays = new Map<string, number>();
  /** Legacy session sockets the hub stopped reading after a refusal (cas-2036). */
  private readonly unreadLegacySockets = new WeakSet<WebSocketRoute>();
  /** client_refs the double refused with `upstream_unavailable`, in order. */
  readonly upstreamRefusals: string[] = [];
  /** Turns pushed live, replayed in history like a real hub after a reload. */
  private readonly live = new Map<string, { messages: Array<Record<string, unknown>>; replies: Array<Record<string, unknown>> }>();

  constructor(private readonly page: Page, private readonly options: DoubleOptions) {}

  machine(id: string): Machine {
    const found = this.options.machines.find((m) => m.id === id);
    if (!found) throw new Error(`hub double: unknown machine ${id}`);
    return found;
  }

  /** Install routes; call before the first navigation. */
  async install(): Promise<void> {
    const page = this.page;
    // /v1/events is a long-lived event stream the bundle reads with fetch.
    await page.addInitScript(() => {
      const original = window.fetch;
      // Outages (journey network cells): a machine the page cannot reach
      // refuses new event streams, and an outage that resets connections
      // errors the open ones; a stalled one just goes quiet.
      const down = new Set<string>();
      const streams = new Map<string, Set<ReadableStreamDefaultController<Uint8Array>>>();
      const w = window as unknown as { __journeyOutage: (host: string, isDown: boolean, reset: boolean) => void };
      w.__journeyOutage = (host, isDown, reset) => {
        if (isDown) down.add(host); else down.delete(host);
        if (isDown && reset) {
          for (const controller of streams.get(host) ?? []) { try { controller.error(new TypeError("network changed")); } catch { /* closed */ } }
          streams.delete(host);
        }
      };
      window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
        const url = new URL(String(input instanceof Request ? input.url : input), location.href);
        if (url.hostname.endsWith(".test") && url.pathname === "/v1/events") {
          if (down.has(url.hostname) || !navigator.onLine) return Promise.reject(new TypeError("Failed to fetch"));
          if (init?.signal?.aborted) return Promise.reject(new DOMException("The operation was aborted.", "AbortError"));
          let registered: ReadableStreamDefaultController<Uint8Array> | undefined;
          const body = new ReadableStream<Uint8Array>({
            start(c) {
              registered = c;
              let set = streams.get(url.hostname);
              if (!set) streams.set(url.hostname, (set = new Set()));
              set.add(c);
              c.enqueue(new TextEncoder().encode(": double connected\n\n"));
            },
            cancel() { if (registered) streams.get(url.hostname)?.delete(registered); },
          });
          // As a real fetch does, aborting the request errors its body stream.
          init?.signal?.addEventListener("abort", () => {
            if (!registered) return;
            streams.get(url.hostname)?.delete(registered);
            try { registered.error(new DOMException("The operation was aborted.", "AbortError")); } catch { /* already closed */ }
          });
          return Promise.resolve(new Response(body, { headers: { "content-type": "text/event-stream" } }));
        }
        return original(input, init);
      };
    });
    await page.route("https://*.test/**", (route) => this.hub(route));
    await page.route(`${RELAY}/api/hub/pairing/**`, (route) => this.relay(route));
    await page.routeWebSocket(/\.test\/v1\//, (ws) => (new URL(ws.url()).pathname === "/v1/attach" ? this.machineSocket(ws) : this.socket(ws)));
  }

  /**
   * Take a machine off the network, as losing Wi-Fi or turning Tailscale off
   * does for that address: HTTP requests fail, new sockets fail, and open
   * sockets stall (half-open, the default) or close.
   */
  async down(machineId: string, outage: Outage = {}): Promise<void> {
    this.outages.set(machineId, outage);
    const reset = outage.sockets === "close";
    if (!reset) {
      for (const ws of [...(this.machineSockets.get(machineId) ?? []), ...(this.sessionSocketsByMachine.get(machineId) ?? [])]) this.stalledSockets.add(ws);
    }
    await this.page.evaluate(({ host, reset }) => (window as unknown as { __journeyOutage: (h: string, d: boolean, r: boolean) => void }).__journeyOutage(host, true, reset), { host: `${machineId}.test`, reset });
    if (!reset) return;
    for (const ws of [...(this.machineSockets.get(machineId) ?? []), ...(this.sessionSocketsByMachine.get(machineId) ?? [])]) {
      void ws.close({ code: 1011, reason: "journey: network reset" });
    }
    this.machineSockets.delete(machineId);
    this.sessionSocketsByMachine.delete(machineId);
  }

  /** Bring a machine back. Stalled sockets stay dead, as half-open ones do. */
  async up(machineId: string): Promise<void> {
    this.outages.delete(machineId);
    await this.page.evaluate((host) => (window as unknown as { __journeyOutage: (h: string, d: boolean, r: boolean) => void }).__journeyOutage(host, false, false), `${machineId}.test`);
  }

  private reachable(machineId: string): boolean { return !this.outages.has(machineId); }

  private readonly proofRefusals = new Map<string, { count: number; reason: string; retryable: boolean }>();
  /** Authenticated HTTP requests the double refused with a reasoned 401 (cas-d636). */
  readonly refusedProofs: Array<{ machine: string; path: string; reason: string }> = [];

  /**
   * Refuse a machine's next `count` authenticated HTTP requests with the real
   * hub's reasoned 401 (cas-d636): by default `stale_proof`, a proof signed
   * before the phone slept and sent when it woke.
   */
  refuseProofs(machineId: string, count: number, reason = "stale_proof", retryable = true): void {
    this.proofRefusals.set(machineId, { count, reason, retryable });
  }

  proofRefusalsLeft(machineId: string): number { return this.proofRefusals.get(machineId)?.count ?? 0; }

  /** Seed paired machines in IndexedDB exactly as a completed pairing stores them. */
  async seedPaired(): Promise<void> {
    const machines = (this.options.paired ?? []).map((id) => ({ id, label: this.machine(id).label }));
    await this.page.evaluate(async ({ machines, scopes }) => {
      localStorage.clear();
      const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
      const publicKey = await crypto.subtle.exportKey("jwk", pair.publicKey);
      // Wait for the app to create its database (cas-00ad). Opening it first
      // created an empty version-1 database with no object stores; the app's
      // own open at version 1 then had no upgrade to run, so every later
      // transaction failed with "object store not found". On a loaded machine
      // the app boots late enough for this seed to win the race.
      const deadline = Date.now() + 15_000;
      while (!(await indexedDB.databases()).some((db) => db.name === "cas-commander-v1")) {
        if (Date.now() > deadline) throw new Error("hub double: the app never opened cas-commander-v1; call seedPaired after the page has booted");
        await new Promise((ok) => setTimeout(ok, 50));
      }
      // No version: join the app's database as it is, after any upgrade it runs.
      const db: IDBDatabase = await new Promise((ok, fail) => {
        const req = indexedDB.open("cas-commander-v1");
        req.onsuccess = () => ok(req.result);
        req.onerror = () => fail(req.error);
      });
      if (!db.objectStoreNames.contains("machines")) {
        db.close();
        throw new Error("hub double: cas-commander-v1 has no machines store; the app's schema did not run");
      }
      await new Promise<void>((ok, fail) => {
        const tx = db.transaction("machines", "readwrite");
        for (const m of machines) {
          tx.objectStore("machines").put({
            id: m.id, label: m.label, baseUrl: `https://${m.id}.test`, deviceId: "journey-device",
            credentialId: "journey-credential", credential: "journey-only", expiresAt: "2099-01-01T00:00:00Z",
            scopes, privateKey: pair.privateKey, publicKey,
          });
        }
        tx.oncomplete = () => ok();
        tx.onerror = () => fail(tx.error);
      });
      db.close();
    }, { machines, scopes: SCOPES });
  }

  /** Push a hub message onto a session's open socket. */
  send(session: string, message: Record<string, unknown>): void {
    const ws = this.sockets.get(session);
    if (!ws) throw new Error(`hub double: no socket open for ${session}`);
    if (this.stalledSockets.has(ws)) return;
    ws.send(JSON.stringify(message));
  }

  /** Acknowledge the latest send and answer it as the supervisor. */
  answerLatest(session: string, message: string, extra: Record<string, unknown> = {}): { queued: number; reply: number } {
    const sent = this.sends.at(-1);
    if (!sent) throw new Error("hub double: nothing was sent");
    const queued = this.nextId++;
    const reply = this.nextId++;
    this.send(session, { MessageQueued: { client_ref: sent.client_ref, notification_id: queued, target: sent.target, stamped: true } });
    this.send(session, { OperatorReply: { notification_id: reply, reply_to: queued, message, summary: "", device_id: "journey-device", ...extra } });
    const now = this.machineNow(session);
    this.remember(session).messages.push({ notification_id: queued, target: sent.target, text: sent.text, state: "acknowledged", stamped: true, device_id: "journey-device", session, ...(sent.in_reply_to === undefined || sent.in_reply_to === null ? {} : { reply_to: sent.in_reply_to }), at: now });
    this.remember(session).replies.push({ notification_id: reply, reply_to: queued, message, summary: "", device_id: "journey-device", attachments: [], session, at: now, ...extra });
    return { queued, reply };
  }

  /**
   * The session's daemon link drops while the hub and the machine stay
   * reachable (cas-0653). A SendMessage is then refused the way the real hub
   * refuses it (hub/server.rs): `upstream_unavailable`, retryable, with its
   * client_ref, and the session's stream is closed so the page attaches again.
   */
  upstreamLost(session: string, options: { refusalDelayMs?: number } = {}): void {
    this.upstreamDown.add(session);
    // The round trip a refusal takes on a real network: a message written
    // meanwhile is already on the legacy socket the hub stops reading (cas-2036).
    if (options.refusalDelayMs) this.upstreamRefusalDelays.set(session, options.refusalDelayMs);
    else this.upstreamRefusalDelays.delete(session);
  }

  /**
   * Stop advertising the machine protocol, as a hub that predates it would:
   * after its next reload the page attaches one legacy socket per session
   * (cas-2036).
   */
  useLegacySockets(): void { this.options.multiplex = false; }

  /** The daemon link is back: the next attach carries sends again. */
  upstreamBack(session: string): void { this.upstreamDown.delete(session); }

  /** Refuse a send while the session's upstream is down; its client_ref when refused. */
  private refuseWithoutUpstream(session: string, message: Record<string, Record<string, unknown>>): string | undefined {
    if (!message.SendMessage || !this.upstreamDown.has(session)) return undefined;
    const clientRef = String(message.SendMessage.client_ref);
    this.upstreamRefusals.push(clientRef);
    return clientRef;
  }

  /** Acknowledge the latest send (MessageQueued) without answering it yet. */
  deliverLatest(session: string): number {
    const sent = this.sends.at(-1);
    if (!sent) throw new Error("hub double: nothing was sent");
    const queued = this.nextId++;
    this.send(session, { MessageQueued: { client_ref: sent.client_ref, notification_id: queued, target: sent.target, stamped: true } });
    this.remember(session).messages.push({ notification_id: queued, target: sent.target, text: sent.text, state: "acknowledged", stamped: true, device_id: "journey-device", session, ...(sent.in_reply_to === undefined || sent.in_reply_to === null ? {} : { reply_to: sent.in_reply_to }), at: this.machineNow(session) });
    return queued;
  }

  /** Answer an already-acknowledged send as the supervisor. */
  answerQueued(session: string, queued: number, message: string, extra: Record<string, unknown> = {}): number {
    const reply = this.nextId++;
    this.send(session, { OperatorReply: { notification_id: reply, reply_to: queued, message, summary: "", device_id: "journey-device", ...extra } });
    this.remember(session).replies.push({ notification_id: reply, reply_to: queued, message, summary: "", device_id: "journey-device", attachments: [], session, at: this.machineNow(session), ...extra });
    return reply;
  }

  /** A supervisor message that is not a reply to anything (status, ask, blocker). */
  supervisorSays(session: string, message: string, extra: Record<string, unknown> = {}): number {
    const id = this.nextId++;
    this.send(session, { OperatorReply: { notification_id: id, reply_to: null, message, summary: "", device_id: "journey-device", ...extra } });
    this.remember(session).replies.push({ notification_id: id, reply_to: null, message, summary: "", device_id: "journey-device", attachments: [], session, at: this.machineNow(session), ...extra });
    return id;
  }

  /** The session's machine clock, for the stamps its history replays. */
  private machineNow(session: string): string {
    return new Date(Date.now() + (this.options.clockAheadMs?.[session] ?? 0)).toISOString();
  }

  /** Replayed like the daemon's history page: every row names its session, and a message its in_reply_to (protocol.rs ConversationHistoryMessage). */
  private remember(session: string) {
    let turns = this.live.get(session);
    if (!turns) this.live.set(session, (turns = { messages: [], replies: [] }));
    return turns;
  }

  /** Drop a session's socket as a network failure would. */
  drop(session: string): void {
    const ws = this.sockets.get(session);
    if (!ws) throw new Error(`hub double: no socket open for ${session}`);
    this.sockets.delete(session);
    void ws.close({ code: 1011, reason: "journey: network dropped" });
  }

  /** Refuse reconnects for a session until `release`: an outage that lasts. */
  hold(session: string): void {
    this.held.add(session);
  }

  release(session: string): void {
    this.held.delete(session);
  }

  /** Answer a session's next attach only after `ms`: a relay that takes its time. */
  delayAttach(session: string, ms: number): void {
    this.attachDelays.set(session, ms);
  }

  hasSocket(session: string): boolean {
    return this.sockets.has(session);
  }

  /** Resolves when the next SendMessage arrives. */
  nextSend(): Promise<SentMessage> {
    const count = this.sends.length;
    return new Promise((ok) => {
      const check = () => (this.sends.length > count ? ok(this.sends[count]) : this.waiters.push(check));
      check();
    });
  }

  private sessionsFor(machineId: string): Session[] {
    return this.machine(machineId).sessions;
  }

  private async hub(route: Route): Promise<void> {
    const url = new URL(route.request().url());
    const machineId = url.hostname.replace(/\.test$/, "");
    const path = url.pathname;
    const method = route.request().method();
    if (!this.reachable(machineId)) return route.abort("internetdisconnected");
    if (path === "/v1/health") return route.fulfill({ json: { ok: true } });
    const refusal = this.proofRefusals.get(machineId);
    if (refusal && refusal.count > 0 && route.request().headers()["dpop"]) {
      refusal.count -= 1;
      this.refusedProofs.push({ machine: machineId, path, reason: refusal.reason });
      return route.fulfill({
        status: 401,
        headers: { "www-authenticate": `DPoP error="${refusal.retryable ? "invalid_dpop_proof" : "invalid_token"}", error_description="${refusal.reason}"` },
        json: { error: "unauthorized", reason: refusal.reason, retryable: refusal.retryable, server_time: Math.floor(Date.now() / 1000) },
      });
    }
    if (path === "/v1/machine") {
      const capabilities = ["session_index", "daemon_attach", "machine_events", ...(this.options.multiplex ? ["machine_multiplex_v2"] : [])];
      return route.fulfill({ json: { schema_version: 1, version: "journey-double", capabilities } });
    }
    if (path === "/v1/sessions") return route.fulfill({ json: { freshness_threshold_secs: 30, sessions: this.sessionsFor(machineId) } });
    if (path === "/v1/auth/pairing/exchange" && method === "POST") {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      this.exchanges.push(body);
      this.exchangeOrigins.push(url.origin);
      const requested = (body.requested_scopes as string[]) ?? [];
      return route.fulfill({
        status: 201,
        json: { device_id: "journey-device", credential_id: "journey-credential", credential: "journey-only", expires_at: "2099-01-01T00:00:00Z", scopes: requested },
      });
    }
    if (path === "/v1/auth/websocket-ticket") return route.fulfill({ json: { ticket: "journey-ticket" } });
    // cassy#910: a signed view URL for an artifact the session published. An
    // id starting `art-local` was never uploaded to Cloud.
    const artifactView = /^\/v1\/sessions\/[^/]+\/artifacts\/([^/]+)\/url$/.exec(path);
    if (artifactView) {
      const id = decodeURIComponent(artifactView[1]!);
      this.artifactRequests.push(id);
      if (id.startsWith("art-local")) return route.fulfill({ status: 409, json: { error: "artifact_not_in_cloud", status: "local" } });
      // cas-e503: Cloud down behind a reachable machine, and a machine that
      // never answers.
      if (id.startsWith("art-cloud-down")) return route.fulfill({ status: 502, json: { error: "cloud_failed", status: null } });
      if (id.startsWith("art-offline")) return route.abort("connectionrefused");
      return route.fulfill({ json: { artifact_id: id, cloud_artifact_id: `cloud-${id}`, url: `https://store.test/view/${encodeURIComponent(id)}?sig=journey`, expires_at: new Date(Date.now() + 600_000).toISOString(), name: `${id}.pdf`, mime: "application/pdf", size_bytes: 1024 } });
    }
    if (path.endsWith("/lease")) return route.fulfill({ json: { held_by_me: true, controller_label: "Journey browser" } });
    if (path.endsWith("/status")) {
      return route.fulfill({ json: { tasks_in_progress: [{ id: "task-journey", title: "Journey suite", status: "in_progress" }], tasks_ready: [], agents: [] } });
    }
    return route.fulfill({ json: {} });
  }

  private async relay(route: Route): Promise<void> {
    const url = new URL(route.request().url());
    const relay = this.options.relay;
    if (!relay) return route.fulfill({ status: 503, json: { error: "relay not configured for this journey" } });
    const body = (route.request().postDataJSON() ?? {}) as Record<string, unknown>;
    const expiresAt = new Date(Date.now() + 600_000).toISOString();
    if (url.pathname.endsWith("/requests")) {
      this.requestedScopes = (body.requested_scopes as string[]) ?? [];
      return route.fulfill({
        status: 201,
        json: {
          wire_version: 1, expires_in: 600, controller_origin: body.controller_origin, user_code: "KQ7M-4XTR",
          requested_scopes: body.requested_scopes, expires_at: expiresAt, pairing_request_id: "journey-request",
          poll_secret: "journey-secret", interval: 0.2,
        },
      });
    }
    if (url.pathname.endsWith("/requests/poll")) {
      this.polls += 1;
      if (this.polls < relay.claimAfter) return route.fulfill({ status: 202, json: { wire_version: 1, status: "authorization_pending", interval: 0.2, expires_at: expiresAt } });
      if (this.polls < relay.authorizeAfter) return route.fulfill({ status: 202, json: { wire_version: 1, status: "machine_claimed", interval: 0.2, expires_at: expiresAt } });
      const machine = this.machine(relay.machine);
      const origin = new URL(route.request().headers()["origin"] ?? route.request().frame().url()).origin;
      return route.fulfill({
        status: 200,
        json: {
          wire_version: 1, status: "authorized", delivery_id: "journey-delivery",
          invitation: {
            controller_origin: origin, hub_url: `https://${machine.id}.test`, scopes: this.requestedScopes,
            token: "journey-invitation", hub_id: machine.id, machine_label: machine.label, expires_at: expiresAt,
          },
        },
      });
    }
    if (url.pathname.endsWith("/requests/acknowledge")) return route.fulfill({ status: 204 });
    return route.fulfill({ status: 404, json: { error: "unknown relay path" } });
  }

  private socket(ws: WebSocketRoute): void {
    const session = decodeURIComponent(new URL(ws.url()).pathname.split("/")[3] ?? "");
    const machineId = new URL(ws.url()).hostname.replace(/\.test$/, "");
    if (this.held.has(session) || !this.reachable(machineId)) { void ws.close({ code: 1011, reason: "journey: still offline" }); return; }
    this.legacySocketOpens.set(session, (this.legacySocketOpens.get(session) ?? 0) + 1);
    this.sockets.set(session, ws);
    let bucket = this.sessionSocketsByMachine.get(machineId);
    if (!bucket) this.sessionSocketsByMachine.set(machineId, (bucket = new Set()));
    bucket.add(ws);
    // A socket open when its machine went down is half-open: nothing it
    // carries arrives, even after the machine is reachable again.
    const pages = [...(this.options.history?.[session] ?? [])];
    ws.onMessage((data) => {
      if (this.stalledSockets.has(ws) || !this.reachable(machineId)) { this.stalledSockets.add(ws); return; }
      // The real hub stops reading a legacy socket once it refused a send on
      // it (hub/server.rs `proxy_socket` breaks out of its loop): anything the
      // page wrote after the refused message is never read (cas-2036).
      if (this.unreadLegacySockets.has(ws)) return;
      const message = JSON.parse(String(data)) as Record<string, Record<string, unknown>>;
      const refused = this.refuseWithoutUpstream(session, message);
      if (refused !== undefined) {
        this.unreadLegacySockets.add(ws);
        // The real hub's legacy socket: the refusal, then a close (cas-0653).
        const refuse = () => {
          ws.send(JSON.stringify({ error: "upstream_unavailable", retryable: true, message: UPSTREAM_UNAVAILABLE_MESSAGE, client_ref: refused }));
          void ws.close({ code: 1011, reason: "journey: session daemon link is reconnecting" });
        };
        const delay = this.upstreamRefusalDelays.get(session);
        if (delay) setTimeout(refuse, delay); else refuse();
        return;
      }
      this.handleSessionFrame(machineId, session, ws, message, pages);
    });
    const welcome = () => ws.send(JSON.stringify(this.welcomeFor(session)));
    const delay = this.attachDelays.get(session);
    this.attachDelays.delete(session);
    if (delay === undefined) welcome();
    else setTimeout(welcome, delay);
  }

  private welcomeFor(session: string): Record<string, unknown> {
    return {
      Welcome: {
        state: { panes: [{ id: "supervisor", kind: "Supervisor", title: session, focused: true, exited: false }], cols: 100, rows: 28 },
        scrollback: { supervisor: [[...new TextEncoder().encode(PANE_TEXT)]] },
        protocol_version: 3,
        capabilities: ["conversation_history"],
      },
    };
  }

  /**
   * The real hub's machine socket (protocol v2, cas-cli hub/server.rs): a
   * `{proto:2}` handshake, `events` and `pty:<session>` subscriptions, frames
   * wrapped as `{channel, message}`, and health ping/pong.
   */
  private machineSocket(ws: WebSocketRoute): void {
    const machineId = new URL(ws.url()).hostname.replace(/\.test$/, "");
    if (!this.reachable(machineId)) { void ws.close({ code: 1011, reason: "journey: unreachable" }); return; }
    this.machineSocketOpens.set(machineId, (this.machineSocketOpens.get(machineId) ?? 0) + 1);
    let bucket = this.machineSockets.get(machineId);
    if (!bucket) this.machineSockets.set(machineId, (bucket = new Set()));
    bucket.add(ws);
    const subscribed = new Set<string>();
    const channel = (session: string): WebSocketRoute => ({
      send: (text: string) => { if (!this.stalledSockets.has(ws)) ws.send(JSON.stringify({ channel: `pty:${session}`, message: JSON.parse(text) })); },
    }) as unknown as WebSocketRoute;
    ws.onMessage((data) => {
      if (this.stalledSockets.has(ws) || !this.reachable(machineId)) { this.stalledSockets.add(ws); return; }
      const frame = JSON.parse(String(data)) as Record<string, any>;
      if (frame.proto === 2) { ws.send(JSON.stringify({ proto: 2 })); return; }
      if (frame.channel === "health" && typeof frame.ping === "number") {
        this.pongs.set(machineId, (this.pongs.get(machineId) ?? 0) + 1);
        ws.send(JSON.stringify({ channel: "health", pong: frame.ping }));
        return;
      }
      if (frame.channel === "events") return;
      if (typeof frame.channel !== "string" || !frame.channel.startsWith("pty:")) return;
      const session = frame.channel.slice(4);
      if (frame.subscribe) {
        if (subscribed.has(session)) return;
        subscribed.add(session);
        // The session's frames go to this socket from now on, as the hub's
        // per-session viewer does.
        this.sockets.set(session, channel(session));
        ws.send(JSON.stringify({ channel: `pty:${session}`, message: this.welcomeFor(session) }));
        return;
      }
      if (!frame.message) return;
      const refused = this.refuseWithoutUpstream(session, frame.message as Record<string, Record<string, unknown>>);
      if (refused !== undefined) {
        // The real hub's machine channel: the refusal, then the session's
        // stream closes so a new subscribe restarts the upstream (cas-0653).
        ws.send(JSON.stringify({ channel: `pty:${session}`, error: { code: "upstream_unavailable", retryable: true, message: UPSTREAM_UNAVAILABLE_MESSAGE, client_ref: refused } }));
        subscribed.delete(session);
        ws.send(JSON.stringify({ channel: `pty:${session}`, closed: true }));
        return;
      }
      this.handleSessionFrame(machineId, session, channel(session), frame.message as Record<string, Record<string, unknown>>, [...(this.options.history?.[session] ?? [])]);
    });
  }

  private handleSessionFrame(machineId: string, session: string, ws: WebSocketRoute, message: Record<string, Record<string, unknown>>, pages: HistoryPage[]): void {
    if (message.SendMessage) {
      const m = message.SendMessage;
      this.sends.push({
        machine: machineId, session, client_ref: String(m.client_ref), target: String(m.target), text: String(m.text),
        ...(typeof m.in_reply_to === "number" ? { in_reply_to: m.in_reply_to } : {}),
      });
      this.waiters.splice(0).forEach((wake) => wake());
    }
    if (message.ConversationHistoryRequest) {
      const request = message.ConversationHistoryRequest;
      this.historyRequests.push({ session, ...request });
      const page = request.before === undefined ? pages[0] : pages.find((p, i) => i > 0 && pages[i - 1].next_before === request.before);
      let reply: HistoryPage = page ?? { messages: [], replies: [], has_earlier: false };
      const live = this.live.get(session);
      if (request.before === undefined && live) {
        reply = { ...reply, messages: [...reply.messages, ...live.messages], replies: [...reply.replies, ...live.replies] };
      }
      ws.send(JSON.stringify({ ConversationHistory: { request_id: request.request_id, ...reply } }));
    }
  }
}

/** Stub the Web Speech API the composer's voice input uses (src/speech-input.ts). */
export async function installSpeechStub(page: Page): Promise<void> {
  await page.addInitScript(() => {
    type Handler = ((event: unknown) => void) | null;
    class JourneyRecognition {
      continuous = false;
      interimResults = false;
      lang = "en-US";
      onresult: Handler = null;
      onerror: Handler = null;
      onend: Handler = null;
      onstart: Handler = null;
      start() {
        (window as unknown as { __journeySpeech: JourneyRecognition }).__journeySpeech = this;
        this.onstart?.({});
      }
      stop() { this.onend?.({}); }
      abort() { this.onend?.({}); }
    }
    (window as unknown as Record<string, unknown>).SpeechRecognition = undefined;
    (window as unknown as Record<string, unknown>).webkitSpeechRecognition = JourneyRecognition;
  });
}

/** Speak into the stubbed recognizer: one final result, then end. */
export async function speak(page: Page, transcript: string): Promise<void> {
  await page.evaluate((text) => {
    const rec = (window as unknown as { __journeySpeech?: { onresult: ((e: unknown) => void) | null; onend: ((e: unknown) => void) | null } }).__journeySpeech;
    if (!rec) throw new Error("speech stub: recognition was never started");
    const alternative = { transcript: text, confidence: 0.94 };
    const result = Object.assign([alternative], { isFinal: true });
    const results = Object.assign([result], { item: (i: number) => [result][i] });
    rec.onresult?.({ resultIndex: 0, results });
    rec.onend?.({});
  }, transcript);
}
