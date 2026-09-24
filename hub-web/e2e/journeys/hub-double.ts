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
};

const PANE_TEXT = "The supervisor is ready.\r\n";

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
  private polls = 0;
  private requestedScopes: string[] = [];
  private nextId = 1000;
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
      window.fetch = (input: RequestInfo | URL, init?: RequestInit) => {
        const url = new URL(String(input instanceof Request ? input.url : input), location.href);
        if (url.hostname.endsWith(".test") && url.pathname === "/v1/events") {
          const body = new ReadableStream({ start(c) { c.enqueue(new TextEncoder().encode(": double connected\n\n")); } });
          return Promise.resolve(new Response(body, { headers: { "content-type": "text/event-stream" } }));
        }
        return original(input, init);
      };
    });
    await page.route("https://*.test/**", (route) => this.hub(route));
    await page.route(`${RELAY}/api/hub/pairing/**`, (route) => this.relay(route));
    await page.routeWebSocket(/\.test\/v1\//, (ws) => this.socket(ws));
  }

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
    if (path === "/v1/health") return route.fulfill({ json: { ok: true } });
    if (path === "/v1/machine") {
      return route.fulfill({ json: { schema_version: 1, version: "journey-double", capabilities: ["session_index", "daemon_attach", "machine_events"] } });
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
    if (this.held.has(session)) { void ws.close({ code: 1011, reason: "journey: still offline" }); return; }
    const machineId = new URL(ws.url()).hostname.replace(/\.test$/, "");
    this.sockets.set(session, ws);
    const pages = [...(this.options.history?.[session] ?? [])];
    ws.onMessage((data) => {
      const message = JSON.parse(String(data)) as Record<string, Record<string, unknown>>;
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
    });
    ws.send(JSON.stringify({
      Welcome: {
        state: { panes: [{ id: "supervisor", kind: "Supervisor", title: session, focused: true, exited: false }], cols: 100, rows: 28 },
        scrollback: { supervisor: [[...new TextEncoder().encode(PANE_TEXT)]] },
        protocol_version: 3,
        capabilities: ["conversation_history"],
      },
    }));
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
