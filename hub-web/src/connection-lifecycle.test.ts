import { webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import { replaceMachineConnection } from "./connection-lifecycle";
import { HubConnectionSupervisor, type ConnectionState, type HubCallbacks } from "./connection";
import { HEARTBEAT_INTERVAL_MS, MACHINE_RETRY_CEILING_MS, headerConnectionChip } from "./connection-state";
import { createDeviceKey } from "./dpop";
import type { StoredMachine } from "./types";

const supervisors: HubConnectionSupervisor[] = [];
afterEach(() => {
  for (const supervisor of supervisors.splice(0)) supervisor.stop();
  vi.restoreAllMocks();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function storedMachine(credentialId: string): Promise<StoredMachine> {
  return {
    id: "stable-hub-id", label: "Studio Mac", baseUrl: "https://workstation.tail.example",
    deviceId: `device-${credentialId}`, credentialId, credential: `opaque-${credentialId}`,
    expiresAt: new Date(Date.now() + 60_000).toISOString(), scopes: ["machine-read"],
    ...await createDeviceKey(),
  };
}

// Only HTTP, the browser clock and the open SSE stream are controlled. Signing,
// authentication, transitions, heartbeat scheduling and teardown remain real.
function transport(multiplex = false) {
  vi.stubGlobal("crypto", webcrypto);
  vi.stubGlobal("window", globalThis);
  let clock = 0;
  vi.spyOn(performance, "now").mockImplementation(() => clock);
  const requests: { path: string; authorization: string | null }[] = [];
  const streams: { signal: AbortSignal; credential: string | null }[] = [];
  let blocked = false;
  let stalledRefresh = false;
  let eventController: ReadableStreamDefaultController<Uint8Array> | undefined;
  vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = new URL(String(input)).pathname;
    const authorization = new Headers(init?.headers).get("Authorization");
    requests.push({ path, authorization });
    if (path === "/v1/health") return new Response("{}");
    if (blocked) throw new TypeError("Failed to fetch");
    if (authorization?.includes("opaque-needs-pairing")) {
      // Only an explicit refusal can end a pairing.
      return Response.json({ reason: "revoked", retryable: false }, { status: 401 });
    }
    if (path === "/v1/machine") {
      clock += 24;
      return Response.json({ capabilities: multiplex ? ["machine_multiplex_v2"] : [] });
    }
    if (path === "/v1/sessions") {
      if (stalledRefresh && !(init?.signal)) return new Promise<Response>(() => {});
      if (stalledRefresh) return new Promise<Response>((_resolve, reject) => {
        init!.signal!.addEventListener("abort", () => reject(init!.signal!.reason), { once: true });
      });
      clock += 17;
      return Response.json({ sessions: [] });
    }
    if (path === "/v1/events") {
      const signal = init!.signal as AbortSignal;
      streams.push({ signal, credential: authorization });
      return new Response(new ReadableStream<Uint8Array>({
        start(controller) {
          eventController = controller;
          signal.addEventListener("abort", () => controller.error(signal.reason), { once: true });
        },
      }));
    }
    if (path === "/v1/auth/websocket-ticket") return Response.json({ ticket: "transport-ticket" });
    throw new Error(`unexpected transport request: ${path}`);
  }));
  return {
    requests, streams, block: (value: boolean) => { blocked = value; }, elapse: (ms: number) => { clock += ms; },
    stallRefresh: (value: boolean) => { stalledRefresh = value; },
    event: (event: Record<string, unknown> = { kind: "session_added" }) => eventController!.enqueue(new TextEncoder().encode(`data: ${JSON.stringify(event)}\n\n`)),
  };
}

class TransportSocket {
  static readonly OPEN = 1;
  static readonly CONNECTING = 0;
  static instances: TransportSocket[] = [];
  readyState = TransportSocket.CONNECTING;
  binaryType = "";
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((message: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(readonly url: URL) { TransportSocket.instances.push(this); }
  open(): void { this.readyState = TransportSocket.OPEN; this.onopen?.(); }
  receive(message: unknown): void { this.onmessage?.({ data: JSON.stringify(message) } as MessageEvent); }
  close(code = 1000): void { this.readyState = 3; this.onclose?.({ code } as CloseEvent); }
  send(value: string): void { this.sent.push(value); }
}

function supervisor(machine: StoredMachine, onState: HubCallbacks["onState"] = () => {}, onMachineEvent: HubCallbacks["onMachineEvent"] = () => {}): HubConnectionSupervisor {
  const connection = new HubConnectionSupervisor(machine, {
    onState, onSessions: () => {}, onMachineEvent, onSessionState: () => {},
    onOutput: () => {}, onPaneKeyframe: () => {}, onSocketError: () => {},
  });
  supervisors.push(connection);
  return connection;
}

describe("Commander live connection lifecycle", () => {
  it("keeps delivering a flood while the catalog stalls, with one catalog flight (cas-2b3a5)", async () => {
    const hub = transport();
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("burst"), () => {}, event => events.push(event));
    connection.start(); await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.filter(row => row.path === "/v1/sessions").length;
    hub.stallRefresh(true);
    for (let i = 1; i <= 100; i++) hub.event({ kind: "session_added", sequence: i, revision: 0 });
    await vi.waitFor(() => expect(events).toHaveLength(100));
    expect(hub.requests.filter(row => row.path === "/v1/sessions").length - before).toBe(1);
  });
  it("resyncs gaps, accepts replay revisions, and resets on a new epoch (cas-2b3a5)", async () => {
    const hub = transport();
    const events: Record<string, unknown>[] = [];
    const connection = supervisor(await storedMachine("event-gap"), () => {}, event => events.push(event));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start(); await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.event({ kind: "stream_metadata", epoch: "one", oldest_sequence: 1, latest_sequence: 1 });
    hub.event({ kind: "session_added", sequence: 1, revision: 0 });
    hub.event({ kind: "replay_complete" });
    hub.event({ kind: "session_added", sequence: 1, revision: 1 });
    hub.event({ kind: "session_added", sequence: 3, revision: 0 });
    await vi.waitFor(() => expect(connection.snapshot().cause?.code).toBe("event_sequence_gap"));
    expect(events.map(event => [event.sequence, event.revision])).toEqual([[1, 0], [1, 1], [3, 0]]);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(hub.streams).toHaveLength(2));
    hub.event({ kind: "stream_metadata", epoch: "two", oldest_sequence: 1, latest_sequence: 1 });
    hub.event({ kind: "session_added", sequence: 1, revision: 0 });
    hub.event({ kind: "replay_complete" });
    await vi.waitFor(() => expect(events).toHaveLength(4));
    expect(connection.snapshot().cause?.code).toBe("event_epoch_changed");
  });
  it("clears permission guidance when the same tailnet pairing reconnects (cas-b85a)", async () => {
    const hub = transport();
    const machine = await storedMachine("local-network");
    machine.baseUrl = "https://soundwave-linux.tailf5a734.ts.net";
    const query = vi.fn().mockResolvedValue({ state: "denied" });
    vi.stubGlobal("navigator", { permissions: { query } });
    const connection = supervisor(machine);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    hub.block(true);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().networkAccessHelp).toContain("Allow Local network access"));
    expect(connection.snapshot().authFailure).toBeUndefined();
    query.mockResolvedValue({ state: "granted" });
    hub.block(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(connection.snapshot().networkAccessHelp).toBeUndefined();
    expect(hub.streams[0]!.credential).toContain("opaque-local-network");
  });
  it("clears permission remediation when the hub explicitly refuses the pairing (cas-b85a)", async () => {
    const hub = transport();
    const machine = await storedMachine("needs-pairing");
    machine.baseUrl = "https://soundwave-linux.tailf5a734.ts.net";
    vi.stubGlobal("navigator", { permissions: { query: vi.fn().mockResolvedValue({ state: "denied" }) } });
    const connection = supervisor(machine);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    hub.block(true);
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().networkAccessHelp).toContain("Allow Local network access"));
    hub.block(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().authFailure).toBe("revoked"));
    expect(connection.snapshot().networkAccessHelp).toBeUndefined();
    const requests = hub.requests.length;
    await vi.advanceTimersByTimeAsync(10_000);
    expect(hub.requests).toHaveLength(requests);
  });
  it("bounds an unanswered event catalog refresh to the probe deadline (cas-b85a)", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("refresh-deadline"));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    vi.spyOn(AbortSignal, "timeout").mockImplementation(ms => {
      const controller = new AbortController();
      setTimeout(() => controller.abort(new DOMException("timed out", "TimeoutError")), ms);
      return controller.signal;
    });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.length;
    hub.stallRefresh(true);
    hub.event();
    await vi.waitFor(() => expect(hub.requests.length).toBeGreaterThan(before));
    await vi.advanceTimersByTimeAsync(3_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    hub.stallRefresh(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(hub.streams).toHaveLength(2);
  });
  it("aborts a pending event catalog refresh when the network goes offline (cas-b85a)", async () => {
    const hub = transport();
    const events = new EventTarget();
    vi.stubGlobal("addEventListener", events.addEventListener.bind(events));
    vi.stubGlobal("removeEventListener", events.removeEventListener.bind(events));
    vi.stubGlobal("document", new EventTarget());
    const connection = supervisor(await storedMachine("stalled-refresh"));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const before = hub.requests.length;
    hub.stallRefresh(true);
    hub.event();
    await vi.waitFor(() => expect(hub.requests.length).toBeGreaterThan(before));
    events.dispatchEvent(new Event("offline"));
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    hub.stallRefresh(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(hub.streams).toHaveLength(2);
  });

  it("retries a local signing error before fetch without calling it a lost pairing (cas-b85a)", async () => {
    const hub = transport();
    const machine = await storedMachine("signing-error");
    const key = machine.privateKey;
    machine.privateKey = undefined as unknown as CryptoKey;
    const connection = supervisor(machine);
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    expect(connection.snapshot().authFailure).toBeUndefined();
    expect(hub.requests.every(request => request.path === "/v1/health")).toBe(true);
    machine.privateKey = key;
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(hub.streams[0]!.credential).toContain("opaque-signing-error");
  });
  it("retries opaque authenticated failures without discarding an active pairing (cas-b85a)", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("browser-blocked"));
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    hub.block(true);
    for (let beat = 0; beat < 4; beat++) {
      const before = hub.requests.length;
      await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
      await vi.waitFor(() => expect(hub.requests.length).toBeGreaterThan(before));
    }
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("backoff"));
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(connection.snapshot()).toMatchObject({ phase: "backoff", stage: "auth" }));
    expect(connection.snapshot().authFailure).toBeUndefined();
    hub.block(false);
    await vi.advanceTimersByTimeAsync(MACHINE_RETRY_CEILING_MS);
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(hub.streams).toHaveLength(2);
    expect(hub.streams[1]!.credential).toContain("opaque-browser-blocked");
  });
  it("replaces a refused pairing with a real supervisor using the new credential", async () => {
    const hub = transport();
    const prior = await storedMachine("needs-pairing");
    const states = new Map<string, ConnectionState>();
    const old = supervisor(prior, (state) => states.set(prior.id, state));
    const connections = new Map([[prior.id, old]]);
    old.start();
    await vi.waitFor(() => expect(old.snapshot()).toMatchObject({ phase: "failed", stage: "auth", authFailure: "revoked" }));

    const replacement = await storedMachine("replacement");
    const installed = replaceMachineConnection(replacement, connections, states, (machine) => supervisor(machine));
    expect(states.has(prior.id)).toBe(false);
    expect(old.snapshot().phase).toBe("idle");
    expect(connections.get(prior.id)).toBe(installed);
    await vi.waitFor(() => expect(installed.snapshot()).toMatchObject({ phase: "live", stage: "live" }));
    expect(installed.snapshot().authFailure).toBeUndefined();
    expect(hub.streams).toHaveLength(1);
    expect(hub.streams[0]!.credential).toContain("opaque-replacement");
    expect(hub.requests.filter((request) => request.path === "/v1/events")).toEqual([
      { path: "/v1/events", authorization: hub.streams[0]!.credential },
    ]);
  });

  it("aborts the old live transport and stops its heartbeats when replacing it", async () => {
    const hub = transport();
    const prior = await storedMachine("old");
    const old = supervisor(prior);
    const connections = new Map([[prior.id, old]]);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    old.start();
    await vi.waitFor(() => expect(old.snapshot().phase).toBe("live"));
    const installed = replaceMachineConnection(await storedMachine("replacement"), connections, new Map(), (machine) => supervisor(machine));
    await vi.waitFor(() => expect(installed.snapshot().phase).toBe("live"));
    expect(hub.streams[0]!.signal.aborted).toBe(true);
    expect(hub.streams[1]!.signal.aborted).toBe(false);
    const boundary = hub.requests.length;
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(installed.snapshot().latencyMs).toBe(41));
    expect(hub.requests.slice(boundary).map((request) => request.authorization)).toEqual([
      hub.streams[1]!.credential, hub.streams[1]!.credential,
    ]);
    expect(old.snapshot().phase).toBe("idle");
  });

  it("shows Checking before the first heartbeat, then the measured round trip", async () => {
    const hub = transport();
    const connection = supervisor(await storedMachine("latency"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    expect(connection.snapshot().latencyMs).toBeUndefined();
    expect(headerConnectionChip(connection.snapshot(), "live", "Live")).toEqual({ state: "checking", text: "Checking…" });
    const boundary = hub.requests.length;
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(connection.snapshot().latencyMs).toBe(41));
    expect(hub.requests.slice(boundary).map((request) => request.path)).toEqual(["/v1/sessions", "/v1/machine"]);
    expect(headerConnectionChip(connection.snapshot(), "live", "Live")).toEqual({ state: "live", text: "41ms" });
  });

  it("keeps multiplexed latency absent until the matching health pong arrives", async () => {
    const hub = transport(true);
    TransportSocket.instances = [];
    vi.stubGlobal("WebSocket", TransportSocket);
    const connection = supervisor(await storedMachine("socket-latency"));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval", "Date"] });
    connection.start();
    await vi.waitFor(() => expect(connection.snapshot().phase).toBe("live"));
    const attached = connection.attach("session-a");
    await vi.waitFor(() => expect(TransportSocket.instances).toHaveLength(1));
    const socket = TransportSocket.instances[0]!;
    socket.open();
    socket.receive({ proto: 2, capabilities: ["pty_binary", "machine_multiplex"] });
    await attached;
    socket.receive({ channel: "pty:session-a", message: { Welcome: {
      session_name: "session-a", state: { focused_pane: "supervisor", panes: [] },
      protocol_version: 3, capabilities: [],
    } } });
    expect(connection.snapshot().latencyMs).toBeUndefined();
    await vi.advanceTimersByTimeAsync(HEARTBEAT_INTERVAL_MS);
    await vi.waitFor(() => expect(socket.sent.map((frame) => JSON.parse(frame)).some((frame) => frame.channel === "health")).toBe(true));
    const ping = socket.sent.map((frame) => JSON.parse(frame)).find((frame) => frame.channel === "health").ping as number;
    expect(connection.snapshot().latencyMs).toBeUndefined();
    socket.receive({ channel: "health", pong: ping - 1 });
    expect(connection.snapshot().latencyMs).toBeUndefined();
    hub.elapse(24);
    socket.receive({ channel: "health", pong: ping });
    // 17ms for the catalog refresh plus 24ms for the socket's answer.
    expect(connection.snapshot().latencyMs).toBe(41);
    expect(headerConnectionChip(connection.snapshot(), "live", "Live")).toEqual({ state: "live", text: "41ms" });
  });
});
