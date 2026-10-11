// Hub protocol double for the user-journey suite.
//
// The page under test is the production bundle (hub-web/dist). Everything the
// bundle says to a machine's hub (HTTPS on https://<id>.test, WebSockets) and
// to the pairing relay is answered here, at the network boundary. Payload
// shapes follow the hub wire types in src/types.ts and the relay contract in
// src/pairing-relay.ts. Evidence label: "real-bundle, protocol-double".
import type { Page, Route, WebSocketRoute } from "@playwright/test";
import { createHash, webcrypto } from "node:crypto";
import { journeyNow } from "./clock";

export const RELAY = "https://petra-stella-cloud.vercel.app";
export const SCOPES = ["machine-read", "session-read", "pane-read", "pane-input", "message-send", "pane-interrupt"];

export type Session = {
  name: string;
  supervisor: string;
  project_dir: string;
  workers: string[];
  liveness: "live";
  dormant?: boolean;
  /** The session's newest queue row (cas-55a4), as the hub catalog reports it. */
  last_activity_at?: string;
  last_activity?: string;
  /** The project's Cassy Cloud identity (cas-eaa3), as the hub reports it. */
  cloud_project_id?: string;
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
  /** Other sessions' turns beside the newest page (cas-55a4). */
  earlier_messages?: Array<Record<string, unknown>>;
  earlier_replies?: Array<Record<string, unknown>>;
};

export type ProtocolTime = {
  now(): number;
  delay(callback: () => void, ms: number): void;
};

export type DoubleOptions = {
  /** Controlled journeys own the browser and protocol reply clock together. */
  time?: ProtocolTime;
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
  /** Scopes each seeded machine's credential carries; default SCOPES. */
  scopes?: Record<string, string[]>;
  /** New session (cas-0f51): what each machine's /v1/projects, /v1/projects/browse and POST /v1/sessions answer. */
  launch?: Record<string, LaunchWorld>;
  /** cas-a474: each session's fleet, as GET /v1/sessions/<s>/status serves it, mutated by POST /v1/sessions/<s>/operations. */
  fleet?: Record<string, FleetWorld>;
};

/** A session's fleet in the double: agents with a spawn generation, tasks, epics and the focused one (cas-a474). */
export type FleetWorld = {
  agents: Array<{ name: string; status: string; current_task?: string | null; generation: number; role?: string; latest_activity?: { summary: string } }>;
  tasks: Array<{ id: string; title: string; status: string; assignee?: string | null; updated_at: string; tip?: string; branch?: string }>;
  epics: Array<{ id: string; title?: string }>;
  focused_epic: string | null;
  /** Names new workers get, in order. */
  spawnNames: string[];
};

/** The scope each operation needs, as the brief's S1-S3 hub checks it. */
const OPERATION_SCOPE: Record<string, string> = {
  request_merge: "message-send", focus_epic: "factory-operate", spawn_workers: "factory-operate", set_worker_hold: "factory-operate",
  assign_task: "factory-operate", recycle_worker: "factory-manage", shutdown_workers: "factory-manage",
};

/** GET /v1/projects rows as the real hub serves them (hub/projects.rs). */
export type LaunchProject = {
  id: string; name: string; path: string; last_touched_at: string; touch_count: number;
  running_session: string | null; target: Record<string, unknown>;
};

export type LaunchWorld = {
  projects: LaunchProject[];
  browse_roots?: Array<{ id: string; name: string; path: string }>;
  /** Folder listings keyed `<root id>:<path>`. */
  browse?: Record<string, { path: string; entries: Array<Record<string, unknown>>; truncated?: boolean }>;
  /** Refusals keyed by project id or browse path: the real hub's {error, detail} and status. */
  refuse?: Record<string, { status: number; error: string; detail: string }>;
  /** The machine's default supervisor CLI, advertised on /v1/machine. */
  defaultCli?: string;
  /** Session names the machine gives new sessions, in order. */
  names: string[];
  /** GET /v1/launch/profiles per CLI (cas-7b52); absent lists nothing. */
  profiles?: Record<string, { installed: boolean; profiles: Array<{ name: string; logged_in: boolean; is_default: boolean }>; error?: string }>;
  /** Session-list fetches after a start before the new session is listed (it is booting). */
  bootPolls?: number;
};

export type LaunchCall = { machine: string; body: Record<string, unknown>; scopes: string[] };

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
type InstallationRow = { machine: string; device_id: string; credential_id: string; credential_generation: number; credential: string; scopes: string[]; device_label: string; operator_label: string; controller_origin: string; public_key_jwk: JsonWebKey; revoked_at: string | null };
const hash = (value: string) => createHash("sha256").update(value).digest("base64url");
const fingerprint = (key: JsonWebKey) => hash(JSON.stringify({ crv: key.crv, kty: key.kty, x: key.x, y: key.y }));
async function verifyInstallation(key: JsonWebKey, proof: string, transcript: unknown[]): Promise<boolean> {
  try {
    const imported = await webcrypto.subtle.importKey("jwk", key, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    return await webcrypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, imported, Buffer.from(proof, "base64url"), new TextEncoder().encode(JSON.stringify(transcript)));
  } catch { return false; }
}

export class HubDouble {
  readonly sends: SentMessage[] = [];
  readonly exchanges: Array<Record<string, unknown>> = [];
  readonly installations = new Map<string, InstallationRow>();
  /** cas-4634: account enrollments the hub verified, by device ID (absent = unenrolled). */
  readonly accountEnrollments = new Map<string, Record<string, unknown>>();
  readonly staleInstallationRefusals: string[] = [];
  installationRefreshes = 0;
  private readonly installationSecrets = new Set<string>();
  private readonly expiredInstallationSecrets = new Set<string>();
  expireInstallation(deviceId: string): void {
    const device = this.installations.get(deviceId);
    if (!device) throw new Error("unknown installation");
    this.expiredInstallationSecrets.add(device.credential);
  }
  private readonly installationOperations = new Map<string, { candidate: InstallationRow; prior?: InstallationRow; phase: "prepared" | "committed" | "aborted" }>();
  private readonly installationHighwater = new Map<string, number>();
  /** The hub origin each pairing exchange was posted to, in order. */
  readonly exchangeOrigins: string[] = [];
  readonly historyRequests: Array<Record<string, unknown>> = [];
  /** Browser application ACKs, separate from socket forwarding. */
  readonly persistedReplies: Array<{ session: string; notification_id: number }> = [];
  /** Artifact ids Commander asked a signed view URL for (cassy#910). */
  readonly artifactRequests: string[] = [];
  private readonly sockets = new Map<string, WebSocketRoute>();
  private readonly waiters: Array<() => void> = [];
  private readonly protocolWaiters = new Set<() => void>();
  /** Actual legacy connects / multiplex subscriptions, not inferred UI readiness. */
  readonly attaches: string[] = [];
  private readonly held = new Set<string>();
  private readonly attachDelays = new Map<string, number>();
  /** Attaches held until the journey releases them (cas-54ed). */
  private readonly attachGates = new Map<string, Promise<void>>();
  private polls = 0;
  /** Relay polls answered so far: the pairing's own clock (cas-03b7). */
  get relayPolls(): number { return this.polls; }
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
  /**
   * GET /v1/sessions answered, per machine (cas-9772). Every catalog the page
   * renders arrives through one of these, so a journey bounds "the page shows
   * the new catalog" by catalog fetches, not by wall-clock time.
   */
  readonly catalogFetches = new Map<string, number>();
  /** GET /v1/machine answered, per machine: heartbeats and dial probes (cas-9772). */
  readonly machineProbes = new Map<string, number>();
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
  /** Refusal frames actually delivered, distinct from sends received/refused. */
  readonly deliveredRefusals: string[] = [];
  /** POST /v1/sessions/<s>/write-grants calls, in order, with what the double answered (cas-ab04). */
  readonly writeGrants: Array<{ machine: string; session: string; body: Record<string, unknown>; status: number }> = [];
  /** POST /v1/sessions/<s>/operations calls, in order, with what the double answered (cas-a474). */
  readonly operations: Array<{ machine: string; session: string; body: Record<string, unknown>; status: number }> = [];
  private readonly operationOutcomes = new Map<string, Record<string, unknown>>();
  /** DELETE /v1/sessions/<name> calls, in order, with the scopes they carried (cas-55a4). */
  readonly ends: Array<{ machine: string; session: string; scopes: string[] }> = [];
  /** POST /v1/sessions bodies, in order (cas-0f51). */
  readonly launches: LaunchCall[] = [];
  /** Sessions started but still booting: listed after this many more session fetches. */
  private readonly booting = new Map<string, { machine: string; session: Session; polls: number }>();
  /** Every session frame the page sent (SendMessage, InterruptPane, ...), in order (cas-0546). */
  readonly frames: Array<{ machine: string; session: string; kind: string; body: unknown }> = [];
  /**
   * Sessions another device controls, by its label (cas-0546). As the real
   * hub (hub/auth.rs acquire_or_force_lease): a take is refused while it
   * holds, unless forced by a hub-admin pairing, which takes it over.
   */
  private readonly leaseHolders = new Map<string, string>();
  /** POST /v1/sessions/<s>/lease calls, in order, with whether they forced. */
  readonly leaseTakes: Array<{ machine: string; session: string; force: boolean; status: number }> = [];
  /** Turns pushed live, replayed in history like a real hub after a reload. */
  private readonly live = new Map<string, { messages: Array<Record<string, unknown>>; replies: Array<Record<string, unknown>> }>();

  constructor(private readonly page: Page, private readonly options: DoubleOptions) {}

  private now(): number { return this.options.time?.now() ?? journeyNow(); }
  private stamp(offset = 0): string { return new Date(this.now() + offset).toISOString(); }
  private delay(callback: () => void, ms: number): void {
    if (this.options.time) this.options.time.delay(callback, ms);
    else setTimeout(callback, ms);
  }

  /** Catalog fetches answered for one machine, or for every machine. */
  catalogFetchCount(machineId?: string): number {
    if (machineId !== undefined) return this.catalogFetches.get(machineId) ?? 0;
    return [...this.catalogFetches.values()].reduce((sum, count) => sum + count, 0);
  }

  /**
   * The machine's session list changed (cas-9772): announce it on its event
   * stream, as the hub does, and resolve once the page has fetched the
   * catalog again. Commander refetches the catalog on every machine event
   * (connection.ts consumeEvents) with no deadline; waiting for the 5 s
   * heartbeat instead, whose fetch aborts after 3 s, is what timed out on a
   * loaded CI host. A heartbeat fetch that lands first resolves this too.
   *
   * `session_added` and `pane_removed` are what the hub sends when sessions
   * come and go; a `session_removed` would also file an Attention item,
   * which these scaffolding changes must not leave behind.
   */
  async announceCatalog(machineId: string, change: { added?: string[]; removed?: string[] } = {}): Promise<void> {
    const before = this.catalogFetchCount(machineId);
    const events = [
      ...(change.added ?? []).map((session) => ({ kind: "session_added", session })),
      ...(change.removed ?? []).map((session) => ({ kind: "pane_removed", session })),
    ];
    if (!events.length) events.push({ kind: "session_added", session: "" });
    for (const event of events) {
      await this.page.evaluate(
        ([host, data]) => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => number }).__journeyMachineEvent(host, data),
        [`${machineId}.test`, JSON.stringify(event)] as const,
      );
    }
    await this.waitFor(() => this.catalogFetchCount(machineId) > before);
  }

  /** Another device (named `label`) holds control of the session until a take overrides it. */
  holdLease(session: string, label: string): void { this.leaseHolders.set(session, label); }

  /** Resolve on the next matching wire observation, with no polling sleeps. */
  waitFor(observed: () => boolean): Promise<void> {
    if (observed()) return Promise.resolve();
    return new Promise(resolve => {
      const check = () => {
        if (observed()) { this.protocolWaiters.delete(check); resolve(); }
      };
      this.protocolWaiters.add(check);
    });
  }

  private observed(): void { for (const check of this.protocolWaiters) check(); }

  machine(id: string): Machine {
    const found = this.options.machines.find((m) => m.id === id);
    if (!found) throw new Error(`hub double: unknown machine ${id}`);
    return found;
  }

  /** Install routes; call before the first navigation. */
  async install(page = this.page): Promise<void> {
    // /v1/events is a long-lived event stream the bundle reads with fetch.
    await page.addInitScript(() => {
      const original = window.fetch;
      // Outages (journey network cells): a machine the page cannot reach
      // refuses new event streams, and an outage that resets connections
      // errors the open ones; a stalled one just goes quiet.
      const down = new Set<string>();
      const streams = new Map<string, Set<ReadableStreamDefaultController<Uint8Array>>>();
      const w = window as unknown as {
        __journeyOutage: (host: string, isDown: boolean, reset: boolean) => void;
        __journeyMachineEvent: (host: string, data: string) => number;
        __journeyResetEvents: (host: string) => void;
        __journeyEventStreamOpens: Record<string, number>;
      };
      w.__journeyEventStreamOpens = {};
      // Event delivery can flap while HTTP and terminal transports keep working.
      w.__journeyResetEvents = (host) => {
        for (const controller of streams.get(host) ?? []) {
          try { controller.close(); } catch { /* already closed */ }
        }
        streams.delete(host);
      };
      // cas-9772: a machine event on the open streams, as the hub announces
      // a session list change. Returns how many streams carried it.
      w.__journeyMachineEvent = (host, data) => {
        let delivered = 0;
        for (const controller of streams.get(host) ?? []) {
          try { controller.enqueue(new TextEncoder().encode(`data: ${data}\n\n`)); delivered += 1; } catch { /* closed */ }
        }
        return delivered;
      };
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
          w.__journeyEventStreamOpens[url.hostname] = (w.__journeyEventStreamOpens[url.hostname] ?? 0) + 1;
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

  private fleetStatus(fleet: FleetWorld): Record<string, unknown> {
    return {
      agents: fleet.agents,
      tasks_in_progress: fleet.tasks.filter((task) => task.status !== "open"),
      tasks_ready: fleet.tasks.filter((task) => task.status === "open"),
      epics: fleet.epics.map((epic) => ({ ...epic, focused: epic.id === fleet.focused_epic })),
      focused_epic: fleet.focused_epic,
    };
  }

  /**
   * cas-ab04: the hub's write-grant endpoint. factory:manage, a reason and an
   * absolute or ~/ path are required; the double answers like the hub.
   */
  private async writeGrant(route: Route, machineId: string, session: string): Promise<void> {
    const body = route.request().postDataJSON() as Record<string, unknown>;
    const answer = async (status: number, json: Record<string, unknown>) => {
      this.writeGrants.push({ machine: machineId, session, body, status });
      this.observed();
      await route.fulfill({ status, json });
    };
    if (!this.scopesFor(machineId).includes("factory-manage")) return answer(403, { error: "scope_denied", required_scope: "factory:manage" });
    const task = String(body.task ?? "");
    if (body.action === "revoke") {
      const removed = this.writeGrants.filter((call) => call.status === 200 && call.body.action === "grant" && call.body.task === task).length;
      return answer(200, { removed });
    }
    const path = String(body.path ?? "");
    if (!String(body.reason ?? "").trim()) return answer(400, { error: "invalid_write_grant", detail: "a reason is required: it is recorded on the task" });
    if (!(path.startsWith("/") || path.startsWith("~/"))) return answer(400, { error: "invalid_write_grant", detail: `write root \`${path}\` must be an absolute path (or start with ~/)` });
    const resolved = path.startsWith("~/") ? `/home/operator/${path.slice(2)}` : path;
    return answer(200, { grant: { task, path: resolved, modes: String(body.mode ?? "create+edit").split("+"), reason: body.reason, granted_by: "commander-device:journey-device" } });
  }

  /** The brief's operations endpoint: scope, op_id dedupe, `expected` preconditions (409 stale), effects, FleetChanged. */
  private async operation(route: Route, machineId: string, session: string): Promise<void> {
    const body = route.request().postDataJSON() as { op_id: string; op: Record<string, unknown>; expected: Record<string, unknown> };
    const answer = async (status: number, json: Record<string, unknown>) => {
      this.operations.push({ machine: machineId, session, body, status });
      this.observed();
      await route.fulfill({ status, json });
    };
    const fleet = this.options.fleet?.[session];
    if (!fleet) return answer(404, { error: "unknown_session" });
    const kind = String(body.op?.kind ?? "");
    const scope = OPERATION_SCOPE[kind];
    if (!scope) return answer(400, { error: "invalid_operation", detail: `unknown operation ${kind}` });
    if (!this.scopesFor(machineId).includes(scope)) return answer(403, { error: "scope_denied", required_scope: scope.replace("-", ":") });
    const previous = this.operationOutcomes.get(body.op_id);
    if (previous) return answer(200, { op_id: body.op_id, outcome: previous });
    const worker = typeof body.op.worker === "string" ? body.op.worker : Array.isArray(body.op.workers) ? String(body.op.workers[0]) : undefined;
    const agent = worker ? fleet.agents.find((item) => item.name === worker) : undefined;
    const stale = (current: Record<string, unknown>, detail: string) => answer(409, { error: "stale", detail, current });
    if (worker) {
      // As ops::fleet::check_worker_generation answers: {worker, generation}, null when gone.
      if (!agent) return stale({ worker, generation: null }, `${worker} is gone`);
      if (body.expected.generation !== undefined && body.expected.generation !== agent.generation) return stale({ worker, generation: agent.generation }, `${worker} restarted`);
      if (kind === "set_worker_hold" && (agent.status === "held") === Boolean(body.op.hold)) return stale({ held: agent.status === "held" }, `${worker} already ${agent.status}`);
    }
    const task = typeof body.op.task_id === "string" ? fleet.tasks.find((item) => item.id === body.op.task_id) : undefined;
    if (kind === "assign_task") {
      if (!task) return answer(404, { error: "unknown_task" });
      if ((task.assignee ?? null) !== (body.expected.assignee ?? null)) return stale({ assignee: task.assignee ?? null, updated_at: task.updated_at }, "assignee changed");
    }
    if (kind === "request_merge" && task?.status !== "awaiting_merge") return stale({ status: task?.status ?? null }, "no longer awaiting merge");
    if (kind === "focus_epic" && (body.expected.epic_id ?? null) !== fleet.focused_epic) return stale({ epic_id: fleet.focused_epic }, "focus changed");
    let outcome: Record<string, unknown> = { kind };
    if (kind === "set_worker_hold" && agent) agent.status = body.op.hold ? "held" : "active";
    if (kind === "recycle_worker" && agent) { agent.generation += 1; agent.status = "active"; }
    if (kind === "shutdown_workers" && agent) {
      fleet.agents.splice(fleet.agents.indexOf(agent), 1);
      const catalog = this.options.machines.find(machine => machine.id === machineId)?.sessions.find(item => item.name === session);
      if (catalog) catalog.workers = catalog.workers.filter(name => name !== agent.name);
      // Its work in progress goes back to ready; a delivery waiting for merge stays where it is.
      for (const owned of fleet.tasks) if (owned.assignee === agent.name && owned.status === "in_progress") { owned.assignee = null; owned.status = "open"; }
    }
    if (kind === "assign_task" && task) {
      task.assignee = (body.op.assignee as string | null) ?? null;
      task.status = task.assignee ? "in_progress" : "open";
      task.updated_at = this.stamp(0);
      const assignee = fleet.agents.find((item) => item.name === task.assignee);
      if (assignee) assignee.current_task = task.id;
      for (const other of fleet.agents) if (other.name !== task.assignee && other.current_task === task.id) other.current_task = null;
    }
    if (kind === "focus_epic") fleet.focused_epic = String(body.op.epic_id);
    if (kind === "spawn_workers") {
      const names = fleet.spawnNames.splice(0, Number(body.op.count ?? 1));
      for (const name of names) fleet.agents.push({ name, status: "active", generation: 1, current_task: (body.op.task_id as string | undefined) ?? null });
      const catalog = this.options.machines.find(machine => machine.id === machineId)?.sessions.find(item => item.name === session);
      if (catalog) catalog.workers = [...new Set([...catalog.workers, ...names])];
      outcome = { kind, workers: names };
    }
    if (kind === "request_merge") outcome = { kind, notification_id: 7000 + this.operations.length };
    this.operationOutcomes.set(body.op_id, outcome);
    await answer(200, { op_id: body.op_id, outcome });
    // FleetChanged: every connected device refetches status.
    await this.page.evaluate(
      ([host, data]) => (window as unknown as { __journeyMachineEvent: (host: string, data: string) => number }).__journeyMachineEvent(host, data),
      [`${machineId}.test`, JSON.stringify({ kind: "fleet_changed", session })] as const,
    ).catch(() => undefined);
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

  /** The scopes a seeded machine's credential carries. */
  scopesFor(machineId: string): string[] { return this.options.scopes?.[machineId] ?? SCOPES; }

  /** Pair a machine again with other scopes (a new `cas hub pair --scopes` link); call seedPaired and reload after. */
  setScopes(machineId: string, scopes: string[]): void { this.options.scopes = { ...this.options.scopes, [machineId]: scopes }; }

  /** Seed paired machines in IndexedDB exactly as a completed pairing stores them. */
  async seedPaired(): Promise<void> {
    const machines = (this.options.paired ?? []).map((id) => ({ id, label: this.machine(id).label, scopes: this.scopesFor(id) }));
    await this.page.evaluate(async ({ machines }) => {
      localStorage.clear();
      const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
      const publicKey = await crypto.subtle.exportKey("jwk", pair.publicKey);
      // Wait for the app to create its database (cas-00ad). Opening it first
      // created an empty version-1 database with no object stores; the app's
      // own open at version 1 then had no upgrade to run, so every later
      // transaction failed with "object store not found". On a loaded machine
      // the app boots late enough for this seed to win the race.
      const deadline = performance.now() + 15_000;
      while (!(await indexedDB.databases()).some((db) => db.name === "cas-commander-v1")) {
        if (performance.now() > deadline) throw new Error("hub double: the app never opened cas-commander-v1; call seedPaired after the page has booted");
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
            scopes: m.scopes, privateKey: pair.privateKey, publicKey,
          });
        }
        tx.oncomplete = () => ok();
        tx.onerror = () => fail(tx.error);
      });
      db.close();
    }, { machines });
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
    this.observed();
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
    return this.stamp(this.options.clockAheadMs?.[session] ?? 0);
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

  /**
   * Hold the session's next attach until the returned release is called
   * (cas-54ed): a step that must act while the opening card is up waits on
   * the page, not on a delay a loaded host can outrun.
   */
  holdAttach(session: string): () => void {
    let release!: () => void;
    this.attachGates.set(session, new Promise<void>((ok) => { release = ok; }));
    return () => release();
  }

  /** Send the welcome now, after a held attach is released, or after a set delay. */
  private welcomeWhenReady(session: string, welcome: () => void): void {
    const gate = this.attachGates.get(session);
    this.attachGates.delete(session);
    const delay = this.attachDelays.get(session);
    this.attachDelays.delete(session);
    if (gate) void gate.then(welcome);
    else if (delay === undefined) welcome();
    else this.delay(welcome, delay);
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
    if (path === "/v1/auth/pairing/protocol") return route.fulfill({ json: { installation_protocol: 1 } });
    if (path === "/v1/health") return route.fulfill({ json: { ok: true, installation_protocol: 1 } });
    if (path === "/v1/auth/pairing/commit" || path === "/v1/auth/pairing/abort") return this.installationAction(route, machineId, path.endsWith("commit"));
    const active = [...this.installations.values()].find((d) => route.request().headers()["authorization"] === `DPoP ${d.credential}` && d.machine === machineId && !d.revoked_at);
    const secret = route.request().headers()["authorization"]?.replace(/^DPoP /, "");
    if (secret && this.installationSecrets.has(secret) && !active) {
      this.staleInstallationRefusals.push(path);
      return route.fulfill({ status: 401, json: { reason: "unknown_credential", retryable: false } });
    }
    if (active && this.expiredInstallationSecrets.has(active.credential) && path !== "/v1/auth/refresh") {
      return route.fulfill({ status: 401, json: { reason: "expired", retryable: false } });
    }
    if (path === "/v1/auth/refresh" && active) {
      this.installationRefreshes++;
      const generation = (this.installationHighwater.get(active.device_id) ?? active.credential_generation) + 1;
      active.credential_generation = generation; active.credential_id = `refresh-${generation}`;
      active.credential = `journey-refreshed-${generation}`;
      this.installationHighwater.set(active.device_id, generation); this.installationSecrets.add(active.credential);
      return route.fulfill({ json: { ...active, expires_at: "2099-01-01T00:00:00Z", account_enrollment: this.accountEnrollments.get(active.device_id) ?? { state: "unenrolled" } } });
    }
    if (path === "/v1/auth/devices") {
      if (!active) return route.fulfill({ status: 401 });
      const devices = [...this.installations.values()].filter((d) => d.machine === machineId && (active.scopes.includes("hub-admin") || d.device_id === active.device_id));
      return route.fulfill({ json: devices.map((d) => ({ device_id: d.device_id, device_label: d.device_label, operator_label: d.operator_label, controller_origin: d.controller_origin, credential_generation: d.credential_generation, key_fingerprint: fingerprint(d.public_key_jwk), revoked_at: d.revoked_at, issued_at: "2026-10-05T12:00:00Z", last_used_at: "2026-10-05T14:00:00Z", account_enrollment: this.accountEnrollments.get(d.device_id) ?? { state: "unenrolled" } })) });
    }
    const revoke = /^\/v1\/auth\/devices\/([^/]+)\/revoke$/.exec(path);
    if (revoke) {
      if (!active || (!active.scopes.includes("hub-admin") && active.device_id !== revoke[1])) return route.fulfill({ status: 401 });
      const row = this.installations.get(revoke[1]!);
      if (!row || row.machine !== machineId) return route.fulfill({ status: 404 });
      row.revoked_at = "2026-10-05T14:00:00Z";
      return route.fulfill({ status: 204 });
    }
    const refusal = this.proofRefusals.get(machineId);
    if (refusal && refusal.count > 0 && route.request().headers()["dpop"]) {
      refusal.count -= 1;
      this.refusedProofs.push({ machine: machineId, path, reason: refusal.reason });
      this.observed();
      return route.fulfill({
        status: 401,
        headers: { "www-authenticate": `DPoP error="${refusal.retryable ? "invalid_dpop_proof" : "invalid_token"}", error_description="${refusal.reason}"` },
        json: { error: "unauthorized", reason: refusal.reason, retryable: refusal.retryable, server_time: Math.floor(this.now() / 1000) },
      });
    }
    if (path === "/v1/machine") {
      this.machineProbes.set(machineId, (this.machineProbes.get(machineId) ?? 0) + 1);
      this.observed();
      const capabilities = ["session_index", "daemon_attach", "machine_events", ...(this.options.multiplex ? ["machine_multiplex_v2"] : [])];
      const defaultCli = this.options.launch?.[machineId]?.defaultCli;
      return route.fulfill({ json: { schema_version: 1, version: "journey-double", capabilities, ...(defaultCli ? { default_supervisor_cli: defaultCli } : {}) } });
    }
    if (path === "/v1/auth/scopes" && method === "POST") {
      const add = (route.request().postDataJSON() as { add?: string[] }).add;
      const scopes = this.options.scopes?.[machineId] ?? SCOPES;
      // Session launch, and factory:operate as the fleet-operations brief's S2
      // allows it (cas-d382): one self-grantable scope per request, never
      // factory:manage.
      const grant = add?.length === 1 && ["session-launch", "factory-operate"].includes(add[0]!) ? add[0]! : undefined;
      if (!grant) return route.fulfill({ status: 400, json: { error: "invalid_scope" } });
      if (!["pane-input", "message-send", "pane-interrupt"].every((scope) => scopes.includes(scope))) return route.fulfill({ status: 403, json: { error: "scope_denied" } });
      this.setScopes(machineId, [...new Set([...scopes, grant])]);
      return route.fulfill({ json: { scopes: this.options.scopes![machineId] } });
    }
    if (path === "/v1/sessions" && method === "POST") return this.launch(route, machineId);
    // cas-55a4: End session, as hub/server.rs `end_session` answers it.
    const ending = /^\/v1\/sessions\/([^/]+)$/.exec(path);
    if (ending && method === "DELETE") {
      const session = decodeURIComponent(ending[1]!);
      const scopes = this.scopesFor(machineId);
      this.ends.push({ machine: machineId, session, scopes });
      if (!scopes.includes("factory-manage")) return route.fulfill({ status: 403, json: { error: "scope_denied", required_scope: "factory:manage" } });
      const sessions = this.machine(machineId).sessions;
      const index = sessions.findIndex((candidate) => candidate.name === session);
      if (index < 0) return route.fulfill({ status: 404 });
      sessions.splice(index, 1);
      return route.fulfill({ json: { session, outcome: "ended" } });
    }
    if (path === "/v1/sessions") {
      this.tickBooting(machineId);
      const sessions = this.sessionsFor(machineId);
      await route.fulfill({ json: { freshness_threshold_secs: 30, sessions } });
      // Counted once the page has the answer, so a waiter that resumes on it
      // never races the response it is waiting for.
      this.catalogFetches.set(machineId, (this.catalogFetches.get(machineId) ?? 0) + 1);
      this.observed();
      return;
    }
    if (path === "/v1/launch/profiles") return route.fulfill({ json: this.options.launch?.[machineId]?.profiles ?? {} });
    if (path === "/v1/projects") {
      const world = this.options.launch?.[machineId];
      return route.fulfill({ json: { projects: world?.projects ?? [], browse_roots: world?.browse_roots ?? [] } });
    }
    if (path === "/v1/projects/browse") {
      const world = this.options.launch?.[machineId];
      const rootId = url.searchParams.get("root") ?? "";
      const root = world?.browse_roots?.find((candidate) => candidate.id === rootId);
      const listing = world?.browse?.[`${rootId}:${url.searchParams.get("path") ?? ""}`];
      if (!root || !listing) return route.fulfill({ status: 400 });
      return route.fulfill({ json: { root, path: listing.path, entries: listing.entries, truncated: listing.truncated ?? false } });
    }
    if (path === "/v1/auth/pairing/exchange" && method === "POST") {
      const body = route.request().postDataJSON() as Record<string, unknown>;
      this.exchanges.push(body);
      this.exchangeOrigins.push(url.origin);
      if (body.installation) return this.installationExchange(route, machineId, body);
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
      return route.fulfill({ json: { artifact_id: id, cloud_artifact_id: `cloud-${id}`, url: `https://store.test/view/${encodeURIComponent(id)}?sig=journey`, expires_at: this.stamp(600_000), name: `${id}.pdf`, mime: "application/pdf", size_bytes: 1024 } });
    }
    const leased = /^\/v1\/sessions\/([^/]+)\/lease$/.exec(path);
    if (leased) {
      const session = decodeURIComponent(leased[1]!);
      const holder = this.leaseHolders.get(session);
      if (method === "POST") {
        const force = (route.request().postDataJSON() as { force?: boolean } | null)?.force === true;
        const status = !holder ? 200 : !force ? 409 : this.scopesFor(machineId).includes("hub-admin") ? 200 : 403;
        this.leaseTakes.push({ machine: machineId, session, force, status });
        this.observed();
        if (status === 409) return route.fulfill({ status, json: { error: "lease_unavailable" } });
        if (status === 403) return route.fulfill({ status, json: { error: "scope_denied", required_scope: "hub:admin" } });
        this.leaseHolders.delete(session);
      } else if (method === "GET" && holder) {
        return route.fulfill({ json: { held_by_me: false, controller_label: holder } });
      }
      return route.fulfill({ json: { held_by_me: true, controller_label: "Journey browser" } });
    }
    if (path.endsWith("/status")) {
      const session = decodeURIComponent(path.split("/")[3] ?? "");
      const fleet = this.options.fleet?.[session];
      if (fleet) return route.fulfill({ json: this.fleetStatus(fleet) });
      return route.fulfill({ json: { tasks_in_progress: [{ id: "task-journey", title: "Journey suite", status: "in_progress" }], tasks_ready: [], agents: [] } });
    }
    if (path.endsWith("/operations") && method === "POST") return this.operation(route, machineId, decodeURIComponent(path.split("/")[3] ?? ""));
    if (path.endsWith("/write-grants") && method === "POST") return this.writeGrant(route, machineId, decodeURIComponent(path.split("/")[3] ?? ""));
    return route.fulfill({ json: {} });
  }

  private async installationExchange(route: Route, machine: string, body: Record<string, unknown>): Promise<void> {
    const proof = body.installation as { operation_id: string; credential: string; device_id: string | null; expected_generation: number; proof: string; previous_proof: string | null };
    const key = body.public_key_jwk as JsonWebKey;
    const order = ["machine-read", "session-read", "session-launch", "pane-read", "pane-input", "message-send", "pane-interrupt", "factory-operate", "factory-manage", "hub-admin"];
    const scopes = body.requested_scopes as string[];
    const transcript = ["cassy-installation-v1", machine, body.controller_origin, hash(String(body.token)), proof.operation_id, proof.device_id, proof.expected_generation, fingerprint(key), hash(proof.credential), order.filter((s) => scopes.includes(s)), body.device_label, body.operator_label];
    const previous = proof.device_id ? this.installations.get(proof.device_id) : [...this.installations.values()].find((d) => d.machine === machine && d.controller_origin === body.controller_origin && fingerprint(d.public_key_jwk) === fingerprint(key) && !d.revoked_at);
    if (!await verifyInstallation(key, proof.proof, transcript) || (previous && (previous.machine !== machine || previous.controller_origin !== body.controller_origin || previous.revoked_at || (fingerprint(previous.public_key_jwk) !== fingerprint(key) && !await verifyInstallation(previous.public_key_jwk, proof.previous_proof ?? "", transcript))))) return route.fulfill({ status: 401 });
    if ((previous?.credential_generation ?? 0) !== proof.expected_generation) return route.fulfill({ status: 409 });
    const id = previous?.device_id ?? `installation-${this.installations.size + 1}`;
    const generation = (this.installationHighwater.get(id) ?? previous?.credential_generation ?? 0) + 1;
    this.installationHighwater.set(id, generation);
    const candidate: InstallationRow = { machine, device_id: id, credential_id: `generation-${generation}`, credential_generation: generation, credential: proof.credential, scopes, device_label: String(body.device_label), operator_label: String(body.operator_label), controller_origin: String(body.controller_origin), public_key_jwk: key, revoked_at: null };
    this.installationSecrets.add(candidate.credential);
    this.installationOperations.set(proof.operation_id, { candidate, prior: previous && { ...previous }, phase: "prepared" });
    return route.fulfill({ status: 201, json: { ...candidate, expires_at: "2099-01-01T00:00:00Z", account_enrollment: { state: "unenrolled" } } });
  }

  private async installationAction(route: Route, machine: string, commit: boolean): Promise<void> {
    const body = route.request().postDataJSON() as { operation_id: string; controller_origin: string; public_key_jwk: JsonWebKey; pairing_token_hash: string; proof: string };
    const operation = this.installationOperations.get(body.operation_id);
    if (!await verifyInstallation(body.public_key_jwk, body.proof, [`cassy-installation-${commit ? "commit" : "abort"}-v1`, machine, body.controller_origin, body.operation_id, body.pairing_token_hash])) return route.fulfill({ status: 401 });
    if (!operation) return route.fulfill({ status: commit ? 409 : 204 });
    const { candidate, prior } = operation;
    if (candidate.machine !== machine || candidate.controller_origin !== body.controller_origin || fingerprint(candidate.public_key_jwk) !== fingerprint(body.public_key_jwk)) return route.fulfill({ status: 401 });
    const active = this.installations.get(candidate.device_id);
    if (commit) {
      if (operation.phase === "aborted" || (active?.credential_id !== prior?.credential_id && active?.credential_id !== candidate.credential_id)) return route.fulfill({ status: 409 });
      this.installations.set(candidate.device_id, { ...candidate }); operation.phase = "committed";
    } else if (operation.phase !== "aborted") {
      if (operation.phase === "committed") {
        if (active?.credential_id !== candidate.credential_id || active.revoked_at) return route.fulfill({ status: 409 });
        if (prior) this.installations.set(candidate.device_id, prior); else this.installations.delete(candidate.device_id);
      }
      operation.phase = "aborted";
    }
    return route.fulfill({ status: 204 });
  }

  /**
   * POST /v1/sessions as cas-cli hub/server.rs answers it: 403 scope_denied
   * without session-launch; attached for a project that is already running;
   * a refusal the world names; else 202 with a fresh name, and the session is
   * listed once it has booted.
   */
  private async launch(route: Route, machineId: string): Promise<void> {
    const body = (route.request().postDataJSON() ?? {}) as Record<string, unknown>;
    const scopes = this.scopesFor(machineId);
    this.launches.push({ machine: machineId, body, scopes });
    if (!scopes.includes("session-launch")) return route.fulfill({ status: 403, json: { error: "scope_denied", required_scope: "session:launch" } });
    const world = this.options.launch?.[machineId];
    if (!world) return route.fulfill({ status: 404 });
    const target = (body.target ?? {}) as Record<string, unknown>;
    const key = target.kind === "project" ? String(target.id) : String(target.path);
    const refusal = world.refuse?.[key];
    if (refusal) return route.fulfill({ status: refusal.status, json: { error: refusal.error, detail: refusal.detail } });
    const known = world.projects.find((project) => target.kind === "project" && project.id === target.id);
    if (known?.running_session) return route.fulfill({ status: 200, json: { session: known.running_session, attached: true } });
    // hub/server.rs selected_profile: unknown → 400 invalid_profile, logged out → 422 not_logged_in.
    const cliProfiles = world.profiles?.[String(body.supervisor_cli)]?.profiles ?? [];
    let profile: string | undefined;
    if (typeof body.profile === "string") {
      const row = cliProfiles.find((candidate) => candidate.name === body.profile);
      if (!row) return route.fulfill({ status: 400, json: { error: "invalid_profile", detail: "selected profile is unavailable for this CLI" } });
      if (!row.logged_in) return route.fulfill({ status: 422, json: { error: "not_logged_in", detail: "selected profile is unavailable for this CLI" } });
      profile = row.name;
    } else profile = cliProfiles.find((candidate) => candidate.is_default)?.name ?? "main";
    const name = world.names.shift();
    if (!name) return route.fulfill({ status: 500, json: { error: "launch_failed", detail: "journey: no session names left" } });
    const projectDir = known?.path ?? `${world.browse_roots?.find((root) => root.id === target.root_id)?.path ?? "/projects"}/${String(target.path)}`;
    const session: Session = { name, supervisor: name, project_dir: projectDir, workers: [], liveness: "live" };
    if (known) known.running_session = name;
    this.booting.set(name, { machine: machineId, session, polls: world.bootPolls ?? 2 });
    return route.fulfill({ status: 202, json: { session: name, attached: false, placement: "systemd_user_scope", profile } });
  }

  /** A booting session is listed after its polls run out, as its daemon comes up. */
  private tickBooting(machineId: string): void {
    for (const [name, entry] of this.booting) {
      if (entry.machine !== machineId) continue;
      entry.polls -= 1;
      if (entry.polls > 0) continue;
      this.booting.delete(name);
      this.machine(machineId).sessions.push(entry.session);
    }
  }

  private async relay(route: Route): Promise<void> {
    const url = new URL(route.request().url());
    const relay = this.options.relay;
    if (!relay) return route.fulfill({ status: 503, json: { error: "relay not configured for this journey" } });
    const body = (route.request().postDataJSON() ?? {}) as Record<string, unknown>;
    const expiresAt = this.stamp(600_000);
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
      this.observed();
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
          this.deliveredRefusals.push(refused);
          void ws.close({ code: 1011, reason: "journey: session daemon link is reconnecting" });
        };
        const delay = this.upstreamRefusalDelays.get(session);
        if (delay) this.delay(refuse, delay); else refuse();
        return;
      }
      this.handleSessionFrame(machineId, session, ws, message, pages);
    });
    const welcome = () => ws.send(JSON.stringify(this.welcomeFor(session)));
    this.welcomeWhenReady(session, welcome);
    this.attaches.push(session);
    this.observed();
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
    this.observed();
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
        this.observed();
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
        const welcome = () => ws.send(JSON.stringify({ channel: `pty:${session}`, message: this.welcomeFor(session) }));
        this.welcomeWhenReady(session, welcome);
        this.attaches.push(session);
        this.observed();
        return;
      }
      if (!frame.message) return;
      const refused = this.refuseWithoutUpstream(session, frame.message as Record<string, Record<string, unknown>>);
      if (refused !== undefined) {
        // The real hub's machine channel: the refusal, then the session's
        // stream closes so a new subscribe restarts the upstream (cas-0653).
        ws.send(JSON.stringify({ channel: `pty:${session}`, error: { code: "upstream_unavailable", retryable: true, message: UPSTREAM_UNAVAILABLE_MESSAGE, client_ref: refused } }));
        this.deliveredRefusals.push(refused);
        subscribed.delete(session);
        ws.send(JSON.stringify({ channel: `pty:${session}`, closed: true }));
        return;
      }
      this.handleSessionFrame(machineId, session, channel(session), frame.message as Record<string, Record<string, unknown>>, [...(this.options.history?.[session] ?? [])]);
    });
  }

  private handleSessionFrame(machineId: string, session: string, ws: WebSocketRoute, message: Record<string, Record<string, unknown>>, pages: HistoryPage[]): void {
    const kind = typeof message === "string" ? message : Object.keys(message)[0] ?? "";
    this.frames.push({ machine: machineId, session, kind, body: typeof message === "string" ? message : message[kind] });
    this.observed();
    if (message.OperatorReplyPersisted) this.persistedReplies.push({ session, notification_id: Number(message.OperatorReplyPersisted.notification_id) });
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
